//! Keeping a long conversation inside the model's context window.
//!
//! A coding session can outgrow any model. Before a request would overflow, the older part of
//! the conversation is replaced by a short briefing written by the model itself, and the recent
//! part is kept word for word. The same thing happens on demand (`/compact`) and when a provider
//! says the request was too large.

use crate::Settings;
use crate::provider::ChatMessage;
use crate::stream::Stream;
use crate::tools::ToolSet;
use crate::workflow::Completer;
use anyhow::{Result, bail};
use serde_json::Value;

/// Characters that are about one token. A rough rule is enough: it only decides when to compact.
const CHARS_PER_TOKEN: usize = 4;
/// What a message costs beyond its text (role markers and framing).
const MESSAGE_OVERHEAD_TOKENS: u64 = 4;
/// What an attached image costs; providers differ, so this is a middle figure.
const IMAGE_TOKENS: u64 = 1_000;
/// Compaction starts when a request would fill this share of the context window.
pub(crate) const COMPACT_AT_PERCENT: u64 = 80;
/// How much of the conversation stays word for word: this share of its size, at least this many
/// tokens.
const KEEP_PERCENT: u64 = 30;
const MIN_KEEP_TOKENS: u64 = 4_000;
/// A conversation shorter than this is not worth condensing.
const MIN_SUMMARIZED_TOKENS: u64 = 1_000;
/// What the built-in instructions and tool definitions cost, for the status line (the exact
/// size is only known when a request is built).
pub(crate) const SYSTEM_PROMPT_TOKENS: u64 = 3_000;
/// A tool result longer than this is cut when the older conversation is shown to the summarizer.
const TRANSCRIPT_RESULT_CHARS: usize = 2_000;
/// The most text the summarizer is given; the middle of a longer history is left out.
const TRANSCRIPT_CHARS: usize = 240_000;

const SUMMARY_INSTRUCTIONS: &str = "You are condensing the earlier part of a coding session so the work can continue without it. Write a briefing the assistant can rely on: what the user wants and any constraints they gave; what has been done (files created or changed, commands run and what they showed); decisions made and why; the current state, including anything failing; and what remains to do. Keep exact file paths, identifiers, commands and error messages that matter. Be factual and do not invent anything. Reply with the briefing only.";

const SUMMARY_HEADER: &str = "[Summary of the earlier conversation, written to save context]";

/// The words of a message, for estimating and for the summarizer.
fn message_text(message: &ChatMessage) -> String {
    match &message.content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn image_count(message: &ChatMessage) -> u64 {
    match &message.content {
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
            .count() as u64,
        _ => 0,
    }
}

/// An estimate of what a message costs in tokens.
pub(crate) fn message_tokens(message: &ChatMessage) -> u64 {
    let text = message_text(message).chars().count();
    let calls = message.tool_calls.as_ref().map_or(0, |calls| {
        calls.iter().map(|call| call.to_string().len()).sum()
    });
    ((text + calls).div_ceil(CHARS_PER_TOKEN)) as u64
        + MESSAGE_OVERHEAD_TOKENS
        + image_count(message) * IMAGE_TOKENS
}

/// An estimate of what a whole request costs in tokens.
pub(crate) fn estimate_tokens(messages: &[ChatMessage]) -> u64 {
    messages.iter().map(message_tokens).sum()
}

/// The context window of the active model, when the provider reported it.
pub(crate) fn window_tokens(settings: &Settings) -> Option<u64> {
    let model = settings.model.as_deref()?;
    let profile = settings
        .active_provider_id
        .as_deref()
        .and_then(|id| settings.providers.iter().find(|profile| profile.id == id))?;
    profile
        .model_info
        .get(model)
        .or_else(|| {
            profile
                .model_info
                .iter()
                .find(|(id, _)| id.eq_ignore_ascii_case(model))
                .map(|(_, info)| info)
        })
        .and_then(|info| info.context)
        .filter(|window| *window > 0)
}

/// Whether the next request is close enough to the window that the conversation should be
/// condensed first.
pub(crate) fn should_compact(settings: &Settings, messages: &[ChatMessage]) -> bool {
    settings.auto_compact
        && window_tokens(settings)
            .is_some_and(|window| estimate_tokens(messages) * 100 >= window * COMPACT_AT_PERCENT)
}

/// Whether a provider's error says the request was larger than the model accepts.
pub(crate) fn is_context_overflow(error_text: &str) -> bool {
    let lower = error_text.to_ascii_lowercase();
    [
        "context_length_exceeded",
        "context length",
        "context window",
        "maximum context",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "exceeds the maximum number of tokens",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Where the conversation can be cut: the first message that is kept word for word. Everything
/// before it (after any leading system messages) is summarized. A cut never falls between a
/// tool call and its result, and it prefers to fall before a user message. `None` when there is
/// nothing worth condensing.
pub(crate) fn split_point(messages: &[ChatMessage]) -> Option<usize> {
    let start = messages
        .iter()
        .take_while(|message| message.role == "system")
        .count();
    let total = estimate_tokens(&messages[start..]);
    let keep = (total * KEEP_PERCENT / 100).max(MIN_KEEP_TOKENS);
    let boundaries = (start + 1..messages.len())
        .filter(|index| messages[*index].role != "tool")
        .collect::<Vec<_>>();
    let tail_tokens = |from: usize| estimate_tokens(&messages[from..]);
    let fits = |index: &&usize| tail_tokens(**index) <= keep;
    // The earliest cut that keeps within the budget, favouring one before a user message.
    let preferred = boundaries
        .iter()
        .filter(|index| messages[**index].role == "user")
        .find(fits)
        .or_else(|| boundaries.iter().find(fits))
        .copied();
    // When even the shortest tail is over budget (one huge last message), cut as late as possible.
    let cut = preferred.or_else(|| boundaries.last().copied())?;
    // The cut must leave enough to be worth a summary.
    (cut > start && estimate_tokens(&messages[start..cut]) >= MIN_SUMMARIZED_TOKENS).then_some(cut)
}

/// The older conversation as plain text for the summarizer.
fn transcript(messages: &[ChatMessage]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        let text = message_text(message);
        match message.role.as_str() {
            "user" => lines.push(format!("User: {text}")),
            "assistant" => {
                if !text.trim().is_empty() {
                    lines.push(format!("Assistant: {text}"));
                }
                for call in message.tool_calls.iter().flatten() {
                    let name = call
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    let arguments = call
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    lines.push(format!(
                        "Assistant called {name}: {}",
                        shorten(arguments, TRANSCRIPT_RESULT_CHARS)
                    ));
                }
            }
            "tool" => lines.push(format!(
                "Result of {}: {}",
                message.tool_name.as_deref().unwrap_or("tool"),
                shorten(&text, TRANSCRIPT_RESULT_CHARS)
            )),
            _ => {}
        }
    }
    let joined = lines.join("\n\n");
    if joined.chars().count() <= TRANSCRIPT_CHARS {
        return joined;
    }
    // Keep the beginning (the goal) and the end (the latest state) of a very long history.
    let half = TRANSCRIPT_CHARS / 2;
    let head: String = joined.chars().take(half).collect();
    let tail: String = joined.chars().skip(joined.chars().count() - half).collect();
    format!("{head}\n\n[... the middle of this history is left out ...]\n\n{tail}")
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}… [{} more characters]", text.chars().count() - limit)
}

/// What a compaction produced.
pub(crate) struct Compaction {
    /// How many messages after the leading system messages were replaced.
    pub(crate) replaced: usize,
    /// What took their place.
    pub(crate) with: Vec<ChatMessage>,
    /// The whole request afterwards.
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) tokens_before: u64,
    pub(crate) tokens_after: u64,
}

/// Replaces the older part of `messages` with a summary the model writes. Fails when there is
/// nothing to condense or the model answers with nothing.
pub(crate) fn compact(
    completer: &dyn Completer,
    messages: &[ChatMessage],
    stream: &Stream<'_>,
) -> Result<Compaction> {
    let Some(cut) = split_point(messages) else {
        bail!("there is not enough conversation to condense yet");
    };
    let start = messages
        .iter()
        .take_while(|message| message.role == "system")
        .count();
    let request = [
        ChatMessage::system(SUMMARY_INSTRUCTIONS.to_owned()),
        ChatMessage::user_with_images(
            String::new(),
            format!(
                "Condense this earlier part of the session:\n\n{}",
                transcript(&messages[start..cut])
            ),
            Vec::new(),
        ),
    ];
    let completion = completer.complete(&request, ToolSet::None, stream)?;
    let summary = completion.text.trim();
    if summary.is_empty() {
        bail!("the model returned an empty summary");
    }
    let note = format!("{SUMMARY_HEADER}\n{summary}");
    let mut with = vec![ChatMessage::user_with_images(
        note.clone(),
        note,
        Vec::new(),
    )];
    // A user message must not be followed by another one, which some providers refuse.
    if messages[cut].role == "user" {
        with.push(ChatMessage::assistant(
            "Understood. I will continue from that summary.".to_owned(),
        ));
    }
    let mut rebuilt = messages[..start].to_vec();
    rebuilt.extend(with.iter().cloned());
    rebuilt.extend(messages[cut..].iter().cloned());
    Ok(Compaction {
        replaced: cut - start,
        tokens_before: estimate_tokens(messages),
        tokens_after: estimate_tokens(&rebuilt),
        with,
        messages: rebuilt,
    })
}

/// A short human description of what a compaction did.
pub(crate) fn describe(compaction: &Compaction) -> String {
    format!(
        "Condensed {} earlier messages into a summary (about {} → {} tokens).",
        compaction.replaced,
        thousands(compaction.tokens_before),
        thousands(compaction.tokens_after)
    )
}

/// `42,000` as `42k`; smaller numbers as they are.
pub(crate) fn thousands(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Completion;
    use crate::{ModelInfo, ModelProfile, ProviderProfile};
    use std::sync::Mutex;

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user_with_images(text.to_owned(), text.to_owned(), Vec::new())
    }

    fn assistant(text: &str) -> ChatMessage {
        ChatMessage::assistant(text.to_owned())
    }

    fn tool_round(id: &str, result: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage::assistant_tool_calls(
                String::new(),
                vec![serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}
                })],
            ),
            ChatMessage::tool_result(id.to_owned(), "read_file".to_owned(), result.to_owned()),
        ]
    }

    /// Answers every request with the same text and remembers what it was asked.
    struct Reply {
        text: String,
        asked: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl Reply {
        fn new(text: &str) -> Reply {
            Reply {
                text: text.to_owned(),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    impl Completer for Reply {
        fn complete(
            &self,
            messages: &[ChatMessage],
            tools: ToolSet,
            _stream: &Stream<'_>,
        ) -> Result<Completion> {
            assert_eq!(tools, ToolSet::None, "a summary needs no tools");
            self.asked.lock().unwrap().push(messages.to_vec());
            Ok(Completion {
                text: self.text.clone(),
                provider_id: None,
                model_id: "m".to_owned(),
                failed_over: false,
                tool_calls: Vec::new(),
            })
        }
    }

    fn stream() -> (Stream<'static>, &'static std::sync::atomic::AtomicBool) {
        let cancel: &'static std::sync::atomic::AtomicBool =
            Box::leak(Box::new(std::sync::atomic::AtomicBool::new(false)));
        let on_event: &'static dyn Fn(crate::stream::StreamEvent) = Box::leak(Box::new(|_| {}));
        (Stream { on_event, cancel }, cancel)
    }

    fn long_conversation() -> Vec<ChatMessage> {
        let big = "x".repeat(8_000);
        let mut messages = vec![ChatMessage::system("be brief".to_owned())];
        for turn in 0..6 {
            messages.push(user(&format!("question {turn} {big}")));
            messages.extend(tool_round(&format!("call-{turn}"), &big));
            messages.push(assistant(&format!("answer {turn}")));
        }
        messages
    }

    #[test]
    fn tokens_are_estimated_from_text_tool_calls_and_images() {
        assert_eq!(
            message_tokens(&user(&"a".repeat(400))),
            100 + MESSAGE_OVERHEAD_TOKENS
        );
        let with_call = &tool_round("c", "ok")[0];
        assert!(
            message_tokens(with_call) > MESSAGE_OVERHEAD_TOKENS,
            "the call arguments count"
        );
        let image = ChatMessage::user_with_images(
            "look".to_owned(),
            "look".to_owned(),
            vec![serde_json::json!({"type": "image_url", "image_url": {"url": "data:x"}})],
        );
        assert!(message_tokens(&image) >= IMAGE_TOKENS);
        assert_eq!(
            estimate_tokens(&[user("abcd"), user("abcd")]),
            2 * (1 + MESSAGE_OVERHEAD_TOKENS)
        );
    }

    #[test]
    fn the_window_comes_from_what_the_provider_reported_for_the_active_model() {
        let mut settings = Settings::default();
        assert_eq!(window_tokens(&settings), None);
        let mut profile = ProviderProfile {
            id: "p".to_owned(),
            models: vec![ModelProfile {
                id: "Big-Model".to_owned(),
                name: String::new(),
            }],
            ..Default::default()
        };
        profile.model_info.insert(
            "Big-Model".to_owned(),
            ModelInfo {
                context: Some(200_000),
                ..Default::default()
            },
        );
        settings.providers = vec![profile];
        settings.active_provider_id = Some("p".to_owned());
        settings.model = Some("big-model".to_owned());
        assert_eq!(
            window_tokens(&settings),
            Some(200_000),
            "ids match regardless of case"
        );
        settings.model = Some("other".to_owned());
        assert_eq!(window_tokens(&settings), None);
    }

    #[test]
    fn compaction_starts_at_the_threshold_only_when_the_window_is_known_and_the_setting_is_on() {
        let mut settings = Settings::default();
        let mut profile = ProviderProfile {
            id: "p".to_owned(),
            ..Default::default()
        };
        profile.model_info.insert(
            "m".to_owned(),
            ModelInfo {
                context: Some(10_000),
                ..Default::default()
            },
        );
        settings.providers = vec![profile];
        settings.active_provider_id = Some("p".to_owned());
        settings.model = Some("m".to_owned());
        let below = vec![user(&"a".repeat(4 * 7_900))];
        let above = vec![user(&"a".repeat(4 * 8_100))];
        assert!(!should_compact(&settings, &below));
        assert!(should_compact(&settings, &above));
        settings.auto_compact = false;
        assert!(!should_compact(&settings, &above), "switched off");
        settings.auto_compact = true;
        settings.model = None;
        assert!(!should_compact(&settings, &above), "unknown window");
    }

    #[test]
    fn providers_complaints_about_size_are_recognized() {
        for text in [
            "This model's maximum context length is 128000 tokens",
            "context_length_exceeded",
            "prompt is too long: 250000 tokens > 200000 maximum",
            "Request exceeds the context window",
        ] {
            assert!(is_context_overflow(text), "{text}");
        }
        for text in ["invalid API key", "rate limit reached", "usage limit"] {
            assert!(!is_context_overflow(text), "{text}");
        }
    }

    #[test]
    fn the_cut_never_separates_a_tool_call_from_its_result() {
        let messages = long_conversation();
        let cut = split_point(&messages).expect("a cut");
        assert!(cut > 1, "something is summarized");
        assert_ne!(
            messages[cut].role, "tool",
            "a result must stay with its call"
        );
        // Every tool message kept has the call that asked for it just before the cut or after.
        let kept = &messages[cut..];
        for (position, message) in kept.iter().enumerate() {
            if message.role == "tool" {
                assert!(position > 0, "the kept part starts with a result");
            }
        }
    }

    #[test]
    fn the_recent_conversation_is_kept_and_the_older_is_summarized() {
        let messages = long_conversation();
        let (stream, _) = stream();
        let reply = Reply::new("The user asked six questions; files were read.");
        let compaction = compact(&reply, &messages, &stream).expect("compaction");
        assert_eq!(
            compaction.messages[0].role, "system",
            "the system prompt stays first"
        );
        assert!(compaction.messages[1].display.starts_with(SUMMARY_HEADER));
        assert!(compaction.messages[1].display.contains("six questions"));
        let last_original = messages.last().unwrap().display.clone();
        assert_eq!(compaction.messages.last().unwrap().display, last_original);
        assert!(
            compaction.tokens_after < compaction.tokens_before / 2,
            "{}",
            describe(&compaction)
        );
        assert_eq!(
            compaction.messages.len(),
            messages.len() - compaction.replaced + compaction.with.len()
        );
        // The summarizer saw the older conversation, with tool results cut short.
        let asked = reply.asked.lock().unwrap();
        let shown = asked[0][1].display.clone() + &message_text(&asked[0][1]);
        assert!(shown.contains("question 0"), "the start is included");
        assert!(shown.contains("Result of read_file"));
        assert!(
            shown.contains("more characters"),
            "long results are shortened"
        );
    }

    #[test]
    fn a_summary_before_a_user_message_is_followed_by_an_acknowledgement() {
        let messages = long_conversation();
        let (stream, _) = stream();
        let compaction = compact(&Reply::new("briefing"), &messages, &stream).unwrap();
        let after_summary = &compaction.messages[2];
        let next_kept =
            &messages[messages.len() - (compaction.messages.len() - 1 - compaction.with.len())];
        if next_kept.role == "user" {
            assert_eq!(
                after_summary.role, "assistant",
                "never two user messages in a row"
            );
        }
        for pair in compaction.messages.windows(2) {
            assert!(
                !(pair[0].role == "user" && pair[1].role == "user"),
                "{} then {}",
                pair[0].role,
                pair[1].role
            );
        }
    }

    #[test]
    fn a_tiny_conversation_has_nothing_to_condense() {
        let messages = vec![
            ChatMessage::system("s".to_owned()),
            user("hi"),
            assistant("hello"),
        ];
        assert_eq!(split_point(&messages), None);
        let (stream, _) = stream();
        let error = compact(&Reply::new("x"), &messages, &stream)
            .err()
            .expect("error");
        assert!(format!("{error}").contains("not enough"), "{error}");
    }

    #[test]
    fn an_empty_summary_is_an_error_and_changes_nothing() {
        let messages = long_conversation();
        let (stream, _) = stream();
        let error = compact(&Reply::new("   "), &messages, &stream)
            .err()
            .expect("error");
        assert!(format!("{error}").contains("empty summary"), "{error}");
    }

    #[test]
    fn a_huge_history_shows_the_summarizer_its_start_and_its_end() {
        let big = "y".repeat(TRANSCRIPT_CHARS);
        let messages = vec![
            user(&format!("FIRST {big}")),
            assistant(&format!("{big} LAST")),
        ];
        let shown = transcript(&messages);
        assert!(shown.contains("FIRST") && shown.contains("LAST"));
        assert!(shown.contains("middle of this history is left out"));
        assert!(shown.chars().count() < TRANSCRIPT_CHARS + 200);
    }

    #[test]
    fn thousands_are_written_short() {
        assert_eq!(thousands(950), "950");
        assert_eq!(thousands(42_000), "42k");
        assert_eq!(thousands(1_250_000), "1.2M");
    }
}
