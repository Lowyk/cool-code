use std::fmt;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde_json::Value;

use crate::provider::{AgentTurn, ToolCall};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StreamEvent {
    TextDelta(String),
    /// Token counts a provider reported; either may be missing, and later events can refine them.
    Usage {
        input: Option<u64>,
        output: Option<u64>,
    },
}

pub(crate) struct Stream<'a> {
    pub(crate) on_event: &'a dyn Fn(StreamEvent),
    pub(crate) cancel: &'a AtomicBool,
}

impl Stream<'_> {
    pub(crate) fn emit(&self, event: StreamEvent) {
        (self.on_event)(event);
    }
}

/// A stream that ended early (cancelled or failed) after producing `partial` text.
#[derive(Debug)]
pub(crate) struct Interrupted {
    pub(crate) partial: String,
    pub(crate) reason: String,
}

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for Interrupted {}

/// Feeds each server-sent-event `data` payload to `on_data`. Returns `false` if cancelled
/// before the stream finished, `true` at `[DONE]` or end of input.
pub(crate) fn sse_data(
    reader: impl BufRead,
    cancel: &AtomicBool,
    mut on_data: impl FnMut(&str) -> Result<()>,
) -> Result<bool> {
    let mut data: Option<String> = None;
    let mut dispatch = |data: &mut Option<String>| -> Result<Option<bool>> {
        let Some(payload) = data.take() else {
            return Ok(None);
        };
        if cancel.load(Ordering::Relaxed) {
            return Ok(Some(false));
        }
        if payload == "[DONE]" {
            return Ok(Some(true));
        }
        on_data(&payload)?;
        Ok(None)
    };
    for line in reader.lines() {
        let line = line.map_err(|error| anyhow::anyhow!("reading stream: {error}"))?;
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(done) = dispatch(&mut data)? {
                return Ok(done);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            match data.as_mut() {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(rest);
                }
                None => data = Some(rest.to_owned()),
            }
        }
    }
    Ok(dispatch(&mut data)?.unwrap_or(!cancel.load(Ordering::Relaxed)))
}

fn interrupted(partial: &str, reason: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Interrupted {
        partial: partial.to_owned(),
        reason: reason.into(),
    })
}

/// Runs `sse_data`, converting cancellation and read failures into `Interrupted` with the partial text.
fn drive(
    reader: impl BufRead,
    stream: &Stream,
    text: &std::cell::RefCell<String>,
    on_data: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    match sse_data(reader, stream.cancel, on_data) {
        Ok(true) => Ok(()),
        Ok(false) => Err(interrupted(&text.borrow(), "cancelled")),
        Err(error) if error.downcast_ref::<Interrupted>().is_some() => Err(error),
        Err(error) => Err(interrupted(&text.borrow(), error.to_string())),
    }
}

fn emit_usage(stream: &Stream, input: Option<u64>, output: Option<u64>) {
    if input.is_some() || output.is_some() {
        stream.emit(StreamEvent::Usage { input, output });
    }
}

/// Adds up whichever of the named token counters the provider reported.
fn sum_tokens(value: &Value, pointers: &[&str]) -> Option<u64> {
    let counts = pointers
        .iter()
        .filter_map(|pointer| value.pointer(pointer).and_then(Value::as_u64))
        .collect::<Vec<_>>();
    (!counts.is_empty()).then(|| counts.iter().sum())
}

fn push_text(text: &std::cell::RefCell<String>, stream: &Stream, delta: &str) {
    if !delta.is_empty() {
        text.borrow_mut().push_str(delta);
        stream.emit(StreamEvent::TextDelta(delta.to_owned()));
    }
}

fn finish(text: String, tool_calls: Vec<ToolCall>) -> Result<AgentTurn> {
    if text.is_empty() && tool_calls.is_empty() {
        anyhow::bail!("provider response contained neither text nor tool calls");
    }
    Ok(AgentTurn { text, tool_calls })
}

fn parse_arguments(raw: &str) -> Result<Value> {
    if raw.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(raw)
        .map_err(|error| anyhow::anyhow!("parsing tool call arguments: {error}"))
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

pub(crate) fn parse_openai_stream(reader: impl BufRead, stream: &Stream) -> Result<AgentTurn> {
    let text = std::cell::RefCell::new(String::new());
    let mut calls: Vec<PartialCall> = Vec::new();
    drive(reader, stream, &text, |data| {
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return Ok(());
        };
        if let Some(error) = chunk.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| error.to_string());
            return Err(interrupted(
                &text.borrow(),
                format!("provider error: {message}"),
            ));
        }
        emit_usage(
            stream,
            sum_tokens(&chunk, &["/usage/prompt_tokens"]),
            sum_tokens(&chunk, &["/usage/completion_tokens"]),
        );
        let Some(delta) = chunk.pointer("/choices/0/delta") else {
            return Ok(());
        };
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            push_text(&text, stream, content);
        }
        for fragment in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = fragment.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if calls.len() <= index {
                calls.resize_with(index + 1, PartialCall::default);
            }
            let call = &mut calls[index];
            if let Some(id) = fragment.get("id").and_then(Value::as_str) {
                call.id = id.to_owned();
            }
            if let Some(name) = fragment.pointer("/function/name").and_then(Value::as_str) {
                call.name.push_str(name);
            }
            if let Some(arguments) = fragment
                .pointer("/function/arguments")
                .and_then(Value::as_str)
            {
                call.arguments.push_str(arguments);
            }
        }
        Ok(())
    })?;
    let tool_calls = calls
        .into_iter()
        .filter(|call| !call.name.is_empty())
        .map(|call| {
            Ok(ToolCall {
                id: call.id,
                name: call.name,
                arguments: parse_arguments(&call.arguments)?,
                thought_signature: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    finish(text.into_inner(), tool_calls)
}

pub(crate) fn parse_anthropic_stream(reader: impl BufRead, stream: &Stream) -> Result<AgentTurn> {
    let text = std::cell::RefCell::new(String::new());
    let mut blocks: Vec<Option<PartialCall>> = Vec::new();
    drive(reader, stream, &text, |data| {
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            return Ok(());
        };
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "content_block_start" => {
                let block = event.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(Value::as_str) == Some("tool_use") {
                    if blocks.len() <= index {
                        blocks.resize_with(index + 1, || None);
                    }
                    let field = |name| {
                        block
                            .and_then(|b| b.get(name))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    };
                    blocks[index] = Some(PartialCall {
                        id: field("id"),
                        name: field("name"),
                        arguments: String::new(),
                    });
                }
            }
            "content_block_delta" => {
                let delta = event.get("delta");
                match delta.and_then(|d| d.get("type")).and_then(Value::as_str) {
                    Some("text_delta") => {
                        let piece = delta
                            .and_then(|d| d.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        push_text(&text, stream, piece);
                    }
                    Some("input_json_delta") => {
                        if let Some(Some(call)) = blocks.get_mut(index) {
                            let piece = delta
                                .and_then(|d| d.get("partial_json"))
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            call.arguments.push_str(piece);
                        }
                    }
                    _ => {}
                }
            }
            "message_start" => {
                emit_usage(
                    stream,
                    sum_tokens(
                        &event,
                        &[
                            "/message/usage/input_tokens",
                            "/message/usage/cache_read_input_tokens",
                            "/message/usage/cache_creation_input_tokens",
                        ],
                    ),
                    sum_tokens(&event, &["/message/usage/output_tokens"]),
                );
            }
            "message_delta" => {
                emit_usage(
                    stream,
                    sum_tokens(&event, &["/usage/input_tokens"]),
                    sum_tokens(&event, &["/usage/output_tokens"]),
                );
            }
            "error" => {
                let message = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(interrupted(
                    &text.borrow(),
                    format!("provider error: {message}"),
                ));
            }
            _ => {}
        }
        Ok(())
    })?;
    let tool_calls = blocks
        .into_iter()
        .flatten()
        .map(|call| {
            Ok(ToolCall {
                id: call.id,
                name: call.name,
                arguments: parse_arguments(&call.arguments)?,
                thought_signature: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    finish(text.into_inner(), tool_calls)
}

pub(crate) fn parse_google_stream(reader: impl BufRead, stream: &Stream) -> Result<AgentTurn> {
    let text = std::cell::RefCell::new(String::new());
    let mut tool_calls = Vec::new();
    drive(reader, stream, &text, |data| {
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return Ok(());
        };
        if let Some(error) = chunk.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(interrupted(
                &text.borrow(),
                format!("provider error: {message}"),
            ));
        }
        emit_usage(
            stream,
            sum_tokens(&chunk, &["/usageMetadata/promptTokenCount"]),
            sum_tokens(
                &chunk,
                &[
                    "/usageMetadata/candidatesTokenCount",
                    "/usageMetadata/thoughtsTokenCount",
                ],
            ),
        );
        for part in chunk
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(piece) = part.get("text").and_then(Value::as_str) {
                push_text(&text, stream, piece);
            }
            if let Some(call) = part.get("functionCall") {
                let Some(name) = call.get("name").and_then(Value::as_str) else {
                    continue;
                };
                tool_calls.push(ToolCall {
                    id: call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            format!("google-call-{}", uuid::Uuid::new_v4().simple())
                        }),
                    name: name.to_owned(),
                    arguments: call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    thought_signature: part
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
        }
        Ok(())
    })?;
    finish(text.into_inner(), tool_calls)
}

#[cfg(test)]
mod tests {
    use super::{Interrupted, Stream, StreamEvent, parse_openai_stream, sse_data};
    use std::cell::RefCell;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn run_openai(
        fixture: &str,
        cancel: &AtomicBool,
    ) -> (anyhow::Result<crate::provider::AgentTurn>, Vec<StreamEvent>) {
        let events = RefCell::new(Vec::new());
        let on_event = |event| events.borrow_mut().push(event);
        let stream = Stream {
            on_event: &on_event,
            cancel,
        };
        let result = parse_openai_stream(Cursor::new(fixture.to_owned()), &stream);
        (result, events.into_inner())
    }

    fn run_with(
        parse: fn(Cursor<String>, &Stream) -> anyhow::Result<crate::provider::AgentTurn>,
        fixture: &str,
    ) -> (anyhow::Result<crate::provider::AgentTurn>, Vec<StreamEvent>) {
        let cancel = AtomicBool::new(false);
        let events = RefCell::new(Vec::new());
        let on_event = |event| events.borrow_mut().push(event);
        let stream = Stream {
            on_event: &on_event,
            cancel: &cancel,
        };
        let result = parse(Cursor::new(fixture.to_owned()), &stream);
        (result, events.into_inner())
    }

    #[test]
    fn anthropic_text_stream_emits_deltas_and_output_usage() {
        let fixture = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":15}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let (result, events) = run_with(super::parse_anthropic_stream, fixture);
        assert_eq!(result.expect("turn").text, "Hello");
        assert_eq!(
            events
                .iter()
                .find(|event| matches!(event, StreamEvent::TextDelta(_))),
            Some(&StreamEvent::TextDelta("Hel".to_owned()))
        );
        assert!(events.contains(&StreamEvent::Usage {
            input: Some(10),
            output: Some(1)
        }));
        assert!(events.contains(&StreamEvent::Usage {
            input: None,
            output: Some(15)
        }));
    }

    #[test]
    fn anthropic_tool_use_input_is_assembled_from_json_deltas() {
        let fixture = "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\",\"input\":{}}}\n\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"pa\"}}\n\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"th\\\":\\\"a.rs\\\"}\"}}\n\n\
data: {\"type\":\"message_stop\"}\n\n";
        let (result, _) = run_with(super::parse_anthropic_stream, fixture);
        let turn = result.expect("turn");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "toolu_1");
        assert_eq!(turn.tool_calls[0].arguments["path"], "a.rs");
    }

    #[test]
    fn anthropic_error_event_keeps_partial_text() {
        let fixture = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"half\"}}\n\n\
data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let (result, _) = run_with(super::parse_anthropic_stream, fixture);
        let error = result.expect_err("error");
        let interrupted = error.downcast_ref::<Interrupted>().expect("interrupted");
        assert_eq!(interrupted.partial, "half");
        assert!(interrupted.reason.contains("Overloaded"));
    }

    #[test]
    fn google_stream_assembles_text_function_calls_and_usage() {
        let fixture = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hel\"}]}}]}\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo\"},{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"a.rs\"}}}]}}],\"usageMetadata\":{\"candidatesTokenCount\":7}}\r\n\r\n";
        let (result, events) = run_with(super::parse_google_stream, fixture);
        let turn = result.expect("turn");
        assert_eq!(turn.text, "Hello");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.tool_calls[0].arguments["path"], "a.rs");
        assert!(turn.tool_calls[0].id.starts_with("google-call-"));
        assert!(events.contains(&StreamEvent::Usage {
            input: None,
            output: Some(7)
        }));
    }

    #[test]
    fn google_stream_keeps_the_thought_signature_of_a_function_call() {
        let fixture = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"list_files\",\"args\":{}},\"thoughtSignature\":\"sig-abc\"},{\"functionCall\":{\"name\":\"git_status\",\"args\":{}}}]}}]}

";
        let (result, _) = run_with(super::parse_google_stream, fixture);
        let turn = result.expect("turn");
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(
            turn.tool_calls[0].thought_signature.as_deref(),
            Some("sig-abc")
        );
        assert_eq!(turn.tool_calls[1].thought_signature, None);
    }

    #[test]
    fn google_error_payload_keeps_partial_text() {
        let fixture = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"half\"}]}}]}\n\n\
data: {\"error\":{\"code\":500,\"message\":\"Internal\"}}\n\n";
        let (result, _) = run_with(super::parse_google_stream, fixture);
        let error = result.expect_err("error");
        let interrupted = error.downcast_ref::<Interrupted>().expect("interrupted");
        assert_eq!(interrupted.partial, "half");
    }

    #[test]
    fn split_placeholder_is_restored_in_streaming_text() {
        let mapping = vec![("⟦CC-REDACTED-ab⟧".to_owned(), "example-name".to_owned())];
        let mut restorer = super::Restorer::new(&mapping);
        let mut shown = String::new();
        for delta in ["Hi ⟦CC-RED", "ACTED-a", "b⟧, welcome ⟦not a", " token"] {
            let out = restorer.push(delta);
            assert!(!out.contains("CC-RED"), "leaked partial token: {out:?}");
            shown.push_str(&out);
        }
        shown.push_str(&restorer.flush());
        assert_eq!(shown, "Hi example-name, welcome ⟦not a token");
    }

    #[test]
    fn restorer_passes_text_through_without_mapping() {
        let mut restorer = super::Restorer::new(&[]);
        assert_eq!(restorer.push("⟦partial"), "⟦partial");
        assert_eq!(restorer.flush(), "");
    }

    #[test]
    fn openai_usage_reports_prompt_and_completion_tokens() {
        let fixture = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":8}}\n\n\
data: [DONE]\n\n";
        let (_, events) = run_openai(fixture, &AtomicBool::new(false));
        assert!(events.contains(&StreamEvent::Usage {
            input: Some(120),
            output: Some(8)
        }));
    }

    #[test]
    fn anthropic_input_includes_cached_tokens() {
        let fixture = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5,\"cache_read_input_tokens\":100,\"cache_creation_input_tokens\":20,\"output_tokens\":1}}}\n\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
data: {\"type\":\"message_stop\"}\n\n";
        let (_, events) = run_with(super::parse_anthropic_stream, fixture);
        assert!(events.contains(&StreamEvent::Usage {
            input: Some(125),
            output: Some(1)
        }));
    }

    #[test]
    fn google_usage_counts_thinking_tokens_as_output() {
        let fixture = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}],\"usageMetadata\":{\"promptTokenCount\":50,\"candidatesTokenCount\":7,\"thoughtsTokenCount\":30}}\n\n";
        let (_, events) = run_with(super::parse_google_stream, fixture);
        assert!(events.contains(&StreamEvent::Usage {
            input: Some(50),
            output: Some(37)
        }));
    }

    #[test]
    fn sse_data_joins_multiline_payloads_and_skips_other_fields() {
        let input = ": keep-alive\nevent: message\ndata: {\"a\":\ndata: 1}\n\nid: 7\ndata: second\n\ndata: [DONE]\n\ndata: after\n\n";
        let mut seen = Vec::new();
        let finished = sse_data(Cursor::new(input), &AtomicBool::new(false), |data| {
            seen.push(data.to_owned());
            Ok(())
        })
        .expect("parse");
        assert!(finished);
        assert_eq!(seen, vec!["{\"a\":\n1}", "second"]);
    }

    #[test]
    fn openai_text_stream_emits_deltas_and_assembles_text() {
        let fixture = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
data: [DONE]\n\n";
        let (result, events) = run_openai(fixture, &AtomicBool::new(false));
        assert_eq!(result.expect("turn").text, "Hello");
        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta("Hel".to_owned()),
                StreamEvent::TextDelta("lo".to_owned())
            ]
        );
    }

    #[test]
    fn openai_tool_call_split_across_chunks_is_assembled() {
        let fixture = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"pa\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"th\\\": \\\"src/ma\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"in.rs\\\"}\"}}]}}]}\n\n\
data: [DONE]\n\n";
        let (result, _) = run_openai(fixture, &AtomicBool::new(false));
        let turn = result.expect("turn");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "call_1");
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.tool_calls[0].arguments["path"], "src/main.rs");
    }

    #[test]
    fn openai_usage_chunk_emits_usage_and_malformed_lines_are_skipped() {
        let fixture = "data: {not json\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"completion_tokens\":42}}\n\n\
data: [DONE]\n\n";
        let (result, events) = run_openai(fixture, &AtomicBool::new(false));
        assert_eq!(result.expect("turn").text, "ok");
        assert!(events.contains(&StreamEvent::Usage {
            input: None,
            output: Some(42)
        }));
    }

    #[test]
    fn openai_cancelled_stream_reports_partial_text() {
        let cancel = AtomicBool::new(false);
        let events = RefCell::new(Vec::new());
        let on_event = |event: StreamEvent| {
            events.borrow_mut().push(event);
            cancel.store(true, Ordering::Relaxed);
        };
        let stream = Stream {
            on_event: &on_event,
            cancel: &cancel,
        };
        let fixture = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\" never\"}}]}\n\n";
        let error = parse_openai_stream(Cursor::new(fixture), &stream).expect_err("cancelled");
        let interrupted = error.downcast_ref::<Interrupted>().expect("interrupted");
        assert_eq!(interrupted.partial, "partial");
        assert_eq!(interrupted.reason, "cancelled");
    }

    #[test]
    fn openai_error_payload_mid_stream_keeps_partial_text() {
        let fixture = "data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n\n\
data: {\"error\":{\"message\":\"overloaded\"}}\n\n";
        let (result, _) = run_openai(fixture, &AtomicBool::new(false));
        let error = result.expect_err("error");
        let interrupted = error.downcast_ref::<Interrupted>().expect("interrupted");
        assert_eq!(interrupted.partial, "half");
        assert!(interrupted.reason.contains("overloaded"));
    }
}

// Placeholders look like `⟦CC-REDACTED-<32 hex>⟧`; anything longer is not one.
const MAX_PLACEHOLDER_BYTES: usize = 64;

pub(crate) struct Restorer<'a> {
    mapping: &'a [(String, String)],
    pending: String,
}

impl<'a> Restorer<'a> {
    pub(crate) fn new(mapping: &'a [(String, String)]) -> Self {
        Self {
            mapping,
            pending: String::new(),
        }
    }

    /// Returns text that is safe to show; a possibly unfinished placeholder is held back.
    pub(crate) fn push(&mut self, delta: &str) -> String {
        if self.mapping.is_empty() {
            return delta.to_owned();
        }
        self.pending.push_str(delta);
        let hold = self
            .pending
            .rfind('⟦')
            .filter(|start| {
                !self.pending[*start..].contains('⟧')
                    && self.pending.len() - start < MAX_PLACEHOLDER_BYTES
            })
            .unwrap_or(self.pending.len());
        let ready = self.pending[..hold].to_owned();
        self.pending.drain(..hold);
        self.restore(ready)
    }

    pub(crate) fn flush(&mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        self.restore(rest)
    }

    fn restore(&self, mut text: String) -> String {
        for (token, original) in self.mapping {
            text = text.replace(token, original);
        }
        text
    }
}
