use std::fmt;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde_json::Value;

use crate::provider::{AgentTurn, ToolCall};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StreamEvent {
    TextDelta(String),
    Usage(u64),
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
        if let Some(tokens) = chunk
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
        {
            stream.emit(StreamEvent::Usage(tokens));
        }
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
            })
        })
        .collect::<Result<Vec<_>>>()?;
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
        assert!(events.contains(&StreamEvent::Usage(42)));
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
