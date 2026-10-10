use std::{env, sync::OnceLock, thread, time::Duration};

use anyhow::{Context, Result, bail};
use regex::Regex;
use reqwest::blocking::Client;

use crate::stream::{
    Interrupted, Restorer, Stream, StreamEvent, parse_anthropic_stream, parse_google_stream,
    parse_openai_stream,
};
use crate::tools::ToolSet;
use serde::Serialize;
use serde_json::Value;

use crate::{ProviderProfile, Settings, secrets};

#[derive(Clone, Debug)]
pub(crate) struct Completion {
    pub(crate) text: String,
    pub(crate) provider_id: Option<String>,
    pub(crate) model_id: String,
    pub(crate) failed_over: bool,
    pub(crate) tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: Value,
    /// Opaque proof Gemini 3 attaches to a function call; it must be returned with the history.
    pub(crate) thought_signature: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentTurn {
    pub(crate) text: String,
    pub(crate) tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChatMessage {
    pub(crate) role: String,
    pub(crate) content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
    #[serde(skip)]
    pub(crate) tool_name: Option<String>,
    #[serde(skip)]
    pub(crate) display: String,
    /// (tool call id, Gemini thought signature); never serialized into OpenAI/Anthropic bodies.
    #[serde(skip)]
    pub(crate) thought_signatures: Vec<(String, String)>,
}

impl ChatMessage {
    pub(crate) fn user_with_images(display: String, text: String, images: Vec<Value>) -> Self {
        if images.is_empty() {
            return Self {
                role: "user".to_owned(),
                content: Value::String(text),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
                display,
                thought_signatures: Vec::new(),
            };
        }
        let mut parts = vec![serde_json::json!({"type":"text", "text":text})];
        parts.extend(images);
        Self {
            role: "user".to_owned(),
            content: Value::Array(parts),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            display,
            thought_signatures: Vec::new(),
        }
    }

    pub(crate) fn system(content: String) -> Self {
        Self {
            role: "system".to_owned(),
            content: Value::String(content.clone()),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            display: content,
            thought_signatures: Vec::new(),
        }
    }

    pub(crate) fn assistant(content: String) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: Value::String(content.clone()),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            display: content,
            thought_signatures: Vec::new(),
        }
    }

    pub(crate) fn assistant_tool_calls(content: String, calls: Vec<Value>) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: if content.is_empty() {
                Value::Null
            } else {
                Value::String(content.clone())
            },
            tool_calls: Some(calls),
            tool_call_id: None,
            tool_name: None,
            display: content,
            thought_signatures: Vec::new(),
        }
    }

    pub(crate) fn with_thought_signatures(mut self, signatures: Vec<(String, String)>) -> Self {
        self.thought_signatures = signatures;
        self
    }

    pub(crate) fn tool_result(id: String, name: String, content: String) -> Self {
        Self {
            role: "tool".to_owned(),
            content: Value::String(content),
            tool_calls: None,
            tool_call_id: Some(id),
            tool_name: Some(name),
            display: String::new(),
            thought_signatures: Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) fn complete(settings: &Settings, messages: &[ChatMessage]) -> Result<String> {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let ignore = |_| {};
    let stream = Stream {
        on_event: &ignore,
        cancel: &cancel,
    };
    Ok(complete_turn(settings, messages, ToolSet::None, &stream)?.text)
}

fn complete_turn(
    settings: &Settings,
    messages: &[ChatMessage],
    tools: ToolSet,
    stream: &Stream,
) -> Result<AgentTurn> {
    let profile = settings
        .active_provider_id
        .as_deref()
        .and_then(|active_id| {
            settings
                .providers
                .iter()
                .find(|profile| profile.id == active_id)
        });
    if profile.is_some_and(|profile| profile.draft) {
        bail!(
            "the selected provider is a draft; add its API key and at least one model before using it"
        );
    }
    let provider = profile
        .map(|profile| profile.adapter.as_str())
        .or(settings.provider.as_deref())
        .unwrap_or("openai-compatible");
    if !matches!(
        provider,
        "openai-compatible"
            | "openai"
            | "groq"
            | "anthropic"
            | "anthropic-compatible"
            | "google"
            | "chatgpt"
    ) {
        bail!("provider `{provider}` is not supported");
    }

    let model = env::var("HARNESS_MODEL")
        .ok()
        .or_else(|| settings.model.clone())
        .or_else(|| profile.map(|profile| profile.model.clone()))
        .filter(|value| !value.trim().is_empty())
        .context("no model configured; use `coolcode config set --model <model>`")?;
    let base_url = env::var("HARNESS_BASE_URL")
        .ok()
        .or_else(|| profile.and_then(|profile| profile.base_url.clone()))
        .or_else(|| settings.base_url.clone())
        .unwrap_or_else(|| match provider {
            "groq" => "https://api.groq.com/openai/v1".to_owned(),
            "anthropic" => "https://api.anthropic.com/v1".to_owned(),
            "google" => "https://generativelanguage.googleapis.com/v1beta".to_owned(),
            "chatgpt" => crate::chatgpt_auth::API_BASE.to_owned(),
            _ => "https://api.openai.com/v1".to_owned(),
        });
    let risk = privacy_risk(provider, &model, &base_url);
    if let Some(risk) = risk {
        if !settings
            .privacy_acknowledged
            .iter()
            .any(|acknowledged| acknowledged == risk)
        {
            bail!(
                "{risk} request was blocked locally; acknowledge the privacy warning in the TUI first"
            );
        }
        if messages
            .iter()
            .any(|message| contains_image(&message.content))
            && !settings
                .privacy_image_acknowledged
                .iter()
                .any(|acknowledged| acknowledged == risk)
        {
            bail!(
                "image contents for {risk} were not authorized; acknowledge image transmission in the privacy dialog before retrying"
            );
        }
    }
    let key_variable = env::var("HARNESS_API_KEY_ENV")
        .ok()
        .or_else(|| settings.api_key_env.clone())
        .unwrap_or_else(|| {
            match provider {
                "groq" => "GROQ_API_KEY",
                "anthropic" | "anthropic-compatible" => "ANTHROPIC_API_KEY",
                "google" => "GOOGLE_API_KEY",
                _ => "OPENAI_API_KEY",
            }
            .to_owned()
        });
    let stored_key = profile
        .map(|profile: &ProviderProfile| secrets::load(&profile.id))
        .transpose()?
        .flatten();
    // A ChatGPT sign-in has no API key: the saved account is exchanged for a short-lived token.
    let (api_key, account_id) = if provider == "chatgpt" {
        let id = profile
            .map(|profile| profile.id.as_str())
            .context("the ChatGPT sign-in needs its provider entry")?;
        let session = crate::chatgpt_auth::session_for(id, crate::chatgpt_auth::TOKEN_URL)?;
        (session.access_token, session.account_id)
    } else {
        let key = env::var("HARNESS_API_KEY")
            .ok()
            .or(stored_key)
            .or_else(|| env::var(&key_variable).ok())
            .filter(|value| !value.trim().is_empty())
            // A server on this computer (Ollama and the like) does not ask for a key.
            .or_else(|| is_loopback(&base_url).then(String::new))
            .with_context(|| {
                format!("set {key_variable} or HARNESS_API_KEY in your environment")
            })?;
        (key, None)
    };

    // Streamed answers can run long; cap the whole request generously and fail fast on connect.
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build()
        .context("creating HTTP client")?;
    let (safe_messages, redactions) = if risk.is_some() {
        redact_messages(messages, &load_redaction_values()?)
    } else {
        (messages.to_vec(), Vec::new())
    };
    let restorer = std::cell::RefCell::new(Restorer::new(&redactions));
    let forward = |event| match event {
        StreamEvent::TextDelta(delta) => {
            let shown = restorer.borrow_mut().push(&delta);
            if !shown.is_empty() {
                stream.emit(StreamEvent::TextDelta(shown));
            }
        }
        other => stream.emit(other),
    };
    let restoring = Stream {
        on_event: &forward,
        cancel: stream.cancel,
    };
    let api = match (profile.map(|profile| profile.id.as_str()), provider) {
        (Some("openrouter"), _) => crate::effort_support::Api::OpenRouter,
        (_, "anthropic" | "anthropic-compatible") => crate::effort_support::Api::Anthropic,
        (_, "google") => crate::effort_support::Api::Google,
        (_, "chatgpt") => crate::effort_support::Api::ChatGpt,
        _ => crate::effort_support::Api::OpenAiCompatible,
    };
    let request = Request {
        effort: Some(EffortRequest {
            api,
            wanted: settings.effort,
        }),
        client: &client,
        base_url: &base_url,
        api_key: &api_key,
        model: &model,
        messages: &safe_messages,
        tools,
        stream: &restoring,
        account_id: account_id.as_deref(),
    };
    let turn = match provider {
        "anthropic" | "anthropic-compatible" => complete_anthropic(&request),
        "google" => complete_google(&request),
        "chatgpt" => complete_chatgpt(&request),
        _ => complete_openai_compatible(&request),
    };
    let tail = restorer.borrow_mut().flush();
    if !tail.is_empty() {
        stream.emit(StreamEvent::TextDelta(tail));
    }
    let turn = turn.map_err(|error| match error.downcast::<Interrupted>() {
        Ok(interrupted) => anyhow::Error::new(Interrupted {
            partial: restore_redactions(interrupted.partial, &redactions),
            reason: interrupted.reason,
        }),
        Err(error) => error,
    })?;
    let mut tool_calls = turn.tool_calls;
    for call in &mut tool_calls {
        restore_redactions_value(&mut call.arguments, &redactions);
    }
    Ok(AgentTurn {
        text: restore_redactions(turn.text, &redactions),
        tool_calls,
    })
}

pub(crate) fn complete_with_fallback(
    settings: &Settings,
    messages: &[ChatMessage],
    tools: ToolSet,
    stream: &Stream,
) -> Result<Completion> {
    let current_model = env::var("HARNESS_MODEL")
        .ok()
        .or_else(|| settings.model.clone())
        .unwrap_or_default();
    let current_provider = settings.active_provider_id.clone();
    stream.emit(StreamEvent::Attempt);
    match complete_turn(settings, messages, tools, stream) {
        Ok(turn) => Ok(Completion {
            text: turn.text,
            provider_id: current_provider,
            model_id: current_model,
            failed_over: false,
            tool_calls: turn.tool_calls,
        }),
        Err(error) if !should_fall_back(&error) => Err(error),
        Err(error) => {
            let Some(chain_id) = settings.active_chain_id.as_deref() else {
                return Err(error);
            };
            let Some(chain) = settings
                .model_chains
                .iter()
                .find(|chain| chain.id == chain_id)
            else {
                return Err(error);
            };
            let mut last_error = error;
            for member in &chain.members {
                if Some(member.provider_id.as_str()) == current_provider.as_deref()
                    && member.model_id.eq_ignore_ascii_case(&current_model)
                {
                    continue;
                }
                let Some(profile) = settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == member.provider_id && !profile.draft)
                else {
                    continue;
                };
                let mut fallback = settings.clone();
                fallback.active_provider_id = Some(profile.id.clone());
                fallback.provider = Some(profile.adapter.clone());
                fallback.base_url = profile.base_url.clone();
                fallback.model = Some(member.model_id.clone());
                fallback.api_key_env = None;
                stream.emit(StreamEvent::Attempt);
                match complete_turn(&fallback, messages, tools, stream) {
                    Ok(turn) => {
                        return Ok(Completion {
                            text: turn.text,
                            provider_id: Some(profile.id.clone()),
                            model_id: member.model_id.clone(),
                            failed_over: true,
                            tool_calls: turn.tool_calls,
                        });
                    }
                    Err(error) if should_fall_back(&error) => last_error = error,
                    Err(error) => {
                        return Err(error.context(format!(
                            "fallback model {} via {} failed",
                            member.model_id, profile.name
                        )));
                    }
                }
            }
            Err(last_error.context(format!(
                "all available models in chain `{}` reached a usage limit",
                chain.alias
            )))
        }
    }
}

/// A usage-limit error may switch models, but never after text was already shown to the user.
fn should_fall_back(error: &anyhow::Error) -> bool {
    let text_already_shown = error
        .downcast_ref::<Interrupted>()
        .is_some_and(|interrupted| !interrupted.partial.is_empty());
    !text_already_shown && is_usage_limit_error(&format!("{error:#}"))
}

pub(crate) fn is_usage_limit_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "429",
        "rate limit",
        "rate_limit",
        "quota",
        "usage limit",
        "resource exhausted",
        "insufficient_quota",
        "billing limit",
        "credits exhausted",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

pub(crate) fn privacy_risk_for_settings(settings: &Settings) -> Option<&'static str> {
    let profile = settings
        .active_provider_id
        .as_deref()
        .and_then(|id| settings.providers.iter().find(|profile| profile.id == id));
    let provider = profile
        .map(|profile| profile.adapter.as_str())
        .or(settings.provider.as_deref())
        .unwrap_or("openai-compatible");
    let model = env::var("HARNESS_MODEL")
        .ok()
        .or_else(|| settings.model.clone())
        .or_else(|| profile.map(|profile| profile.model.clone()))
        .unwrap_or_default();
    let base_url = env::var("HARNESS_BASE_URL")
        .ok()
        .or_else(|| profile.and_then(|profile| profile.base_url.clone()))
        .or_else(|| settings.base_url.clone())
        .unwrap_or_default();
    privacy_risk(provider, &model, &base_url)
}

pub(crate) fn load_redaction_values() -> Result<Vec<String>> {
    secrets::load("privacy-redaction-values")?
        .map(|json| serde_json::from_str(&json).context("parsing local redaction values"))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(crate) fn save_redaction_values(values: &[String]) -> Result<()> {
    if values.is_empty() {
        secrets::delete("privacy-redaction-values")
    } else {
        secrets::store("privacy-redaction-values", &serde_json::to_string(values)?)
    }
}

fn redact_messages(
    messages: &[ChatMessage],
    custom_values: &[String],
) -> (Vec<ChatMessage>, Vec<(String, String)>) {
    let mut safe_messages = messages.to_vec();
    let mut mapping = Vec::new();
    for message in &mut safe_messages {
        redact_value(&mut message.content, custom_values, &mut mapping);
        if let Some(tool_calls) = &mut message.tool_calls {
            for call in tool_calls {
                redact_value(call, custom_values, &mut mapping);
            }
        }
    }
    (safe_messages, mapping)
}

fn redact_value(value: &mut Value, custom_values: &[String], mapping: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) => *text = redact_text(text, custom_values, mapping),
        Value::Array(parts) => {
            for part in parts {
                redact_value(part, custom_values, mapping);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("image_url") {
                return;
            }
            for (key, part) in object.iter_mut() {
                if key != "image_url" {
                    redact_value(part, custom_values, mapping);
                }
            }
        }
        _ => {}
    }
}

fn redact_text(
    original: &str,
    custom_values: &[String],
    mapping: &mut Vec<(String, String)>,
) -> String {
    let mut text = original.to_owned();
    let mut sorted_custom = custom_values
        .iter()
        .filter(|value| value.chars().count() >= 3)
        .collect::<Vec<_>>();
    sorted_custom.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for value in sorted_custom {
        if text.contains(value) {
            let token = redaction_token(value, mapping);
            text = text.replace(value, &token);
        }
    }
    for pattern in redaction_patterns() {
        text = pattern
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                redaction_token(&captures[0], mapping)
            })
            .into_owned();
    }
    text
}

fn redaction_token(value: &str, mapping: &mut Vec<(String, String)>) -> String {
    if let Some((token, _)) = mapping.iter().find(|(_, original)| original == value) {
        return token.clone();
    }
    let token = format!("⟦CC-REDACTED-{}⟧", uuid::Uuid::new_v4().simple());
    mapping.push((token.clone(), value.to_owned()));
    token
}

fn restore_redactions(mut response: String, mapping: &[(String, String)]) -> String {
    for (token, original) in mapping {
        response = response.replace(token, original);
    }
    response
}

fn restore_redactions_value(value: &mut Value, mapping: &[(String, String)]) {
    match value {
        Value::String(text) => *text = restore_redactions(std::mem::take(text), mapping),
        Value::Array(values) => {
            for value in values {
                restore_redactions_value(value, mapping);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                restore_redactions_value(value, mapping);
            }
        }
        _ => {}
    }
}

pub(crate) fn message_contains_image(message: &ChatMessage) -> bool {
    contains_image(&message.content)
}

fn contains_image(value: &Value) -> bool {
    match value {
        Value::Array(parts) => parts.iter().any(contains_image),
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) == Some("image_url")
                || object.values().any(contains_image)
        }
        _ => false,
    }
}

fn redaction_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| [
        r"(?i)\b(?:sk-[a-z0-9_-]{16,}|gh[pousr]_[a-z0-9_]{20,}|github_pat_[a-z0-9_]{20,}|xox[baprs]-[a-z0-9-]{10,}|AIza[a-z0-9_-]{30,}|AKIA[0-9A-Z]{16})\b",
        r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b",
        r"(?:\+\d[\d(). -]{7,}\d|\b\d{3}[-. ]\d{3}[-. ]\d{4}\b)",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    ].iter().map(|pattern| Regex::new(pattern).expect("static redaction regex is valid")).collect())
}

/// The effort the user chose and the API it will be sent to.
#[derive(Clone, Copy)]
struct EffortRequest {
    api: crate::effort_support::Api,
    wanted: crate::Effort,
}

/// Sends the request built from `body`. If the provider answers 400 and complains about the effort
/// parameter, remembers that the model does not take it and sends once more without it.
fn send_with_effort_fallback(
    mut body: Value,
    sent_effort: Option<(crate::effort_support::Api, &str)>,
    send: impl Fn(&Value) -> Result<reqwest::blocking::Response>,
) -> Result<reqwest::blocking::Response> {
    match send(&body) {
        Err(error)
            if sent_effort.is_some()
                && format!("{error:#}").contains("provider returned 400")
                && crate::effort_support::complains_about_effort(&format!("{error:#}")) =>
        {
            if let Some((api, model)) = sent_effort {
                crate::effort_support::remember_rejected(api, model);
            }
            strip_effort(&mut body);
            send(&body)
        }
        other => other,
    }
}

/// Removes every effort parameter this program may have added.
fn strip_effort(body: &mut Value) {
    if let Some(object) = body.as_object_mut() {
        object.remove("reasoning_effort");
        object.remove("reasoning");
        object.remove("output_config");
        if let Some(config) = object
            .get_mut("generationConfig")
            .and_then(Value::as_object_mut)
        {
            config.remove("thinkingConfig");
            if config.is_empty() {
                object.remove("generationConfig");
            }
        }
    }
}

/// Adds the chosen effort to `body` when the model has that control; returns what was sent so
/// a rejection can be recognized.
fn add_effort<'a>(
    body: &mut Value,
    request: &Request<'a>,
) -> Option<(crate::effort_support::Api, &'a str)> {
    let effort = request.effort?;
    crate::effort_support::apply_effort(body, effort.api, request.model, effort.wanted, &[])
        .then_some((effort.api, request.model))
}

/// Everything a provider adapter needs to run one streamed completion.
#[derive(Clone, Copy)]
struct Request<'a> {
    effort: Option<EffortRequest>,
    client: &'a Client,
    base_url: &'a str,
    api_key: &'a str,
    model: &'a str,
    messages: &'a [ChatMessage],
    tools: ToolSet,
    stream: &'a Stream<'a>,
    /// The ChatGPT account a signed-in request belongs to.
    account_id: Option<&'a str>,
}

/// A turn through the ChatGPT sign-in backend (the Responses format).
fn complete_chatgpt(request: &Request) -> Result<AgentTurn> {
    let Request {
        account_id,
        effort: _,
        client,
        base_url,
        api_key,
        model,
        messages,
        tools,
        stream,
    } = *request;
    let endpoint = format!("{}/responses", base_url.trim_end_matches('/'));
    let mut body = crate::responses::build_request(model, messages, tools, None);
    let sent = add_effort(&mut body, request);
    let session = uuid::Uuid::new_v4().to_string();
    let response = send_with_effort_fallback(body, sent, |body| {
        with_retries(stream.cancel, || {
            let mut builder = client
                .post(&endpoint)
                .bearer_auth(api_key)
                .header("OpenAI-Beta", "responses=experimental")
                .header("originator", "codex_cli_rs")
                .header("session_id", &session)
                .header("Accept", "text/event-stream");
            if let Some(account) = account_id {
                builder = builder.header("chatgpt-account-id", account);
            }
            successful(
                builder
                    .json(body)
                    .send()
                    .context("sending request to the ChatGPT backend")?,
            )
        })
    })?;
    crate::stream::parse_responses_stream(std::io::BufReader::new(response), stream)
}

fn complete_openai_compatible(request: &Request) -> Result<AgentTurn> {
    let Request {
        account_id: _,
        effort: _,
        client,
        base_url,
        api_key,
        model,
        messages,
        tools,
        stream,
    } = *request;
    let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut body = serde_json::json!({ "model": model, "messages": messages, "stream": true });
    if tools.any() {
        body["tools"] = openai_tool_specs(tools);
        body["tool_choice"] = Value::String("auto".to_owned());
    }
    let sent = add_effort(&mut body, request);
    let response = send_with_effort_fallback(body, sent, |body| {
        with_retries(stream.cancel, || {
            successful(
                with_bearer(client.post(&endpoint), api_key)
                    .json(body)
                    .send()
                    .context("sending request to the OpenAI-compatible API")?,
            )
        })
    })?;
    parse_openai_stream(std::io::BufReader::new(response), stream)
}

/// A provider's refusal: its status and error body, and how long it asked us to wait.
#[derive(Debug)]
struct ProviderStatus {
    status: reqwest::StatusCode,
    body: String,
    retry_after: Option<u64>,
}

impl std::fmt::Display for ProviderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "provider returned {}: {}", self.status, self.body)
    }
}

impl std::error::Error for ProviderStatus {}

/// Returns the response for streaming, or the provider's error body as an error.
fn successful(response: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    let body = response.text().unwrap_or_default();
    Err(ProviderStatus {
        status,
        body: body.chars().take(8 * 1024).collect(),
        retry_after,
    }
    .into())
}

/// How many times a request is repeated when the provider is briefly unavailable.
const RETRY_LIMIT: usize = 3;
/// A rate limit that names its own wait is retried this many times at most.
const RATE_LIMIT_RETRIES: usize = 2;
/// The longest a retry will wait.
const MAX_WAIT: Duration = Duration::from_secs(30);

/// How long to wait before retry number `tries` (0 is the first): what the provider asked for,
/// or 1, 2, 4 seconds.
fn backoff(tries: usize, retry_after: Option<u64>) -> Duration {
    retry_after
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(1u64 << tries.min(4)))
        .min(MAX_WAIT)
}

/// Whether a rate-limit answer is a quota or billing wall (not worth waiting for) rather than a
/// brief slow-down.
fn is_hard_limit(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "quota",
        "usage limit",
        "insufficient",
        "billing",
        "credit",
        "exceeded your current",
    ]
    .iter()
    .any(|marker| body.contains(marker))
}

/// How long to wait before repeating a request that failed with `error`, or `None` when
/// repeating it would not help (a wrong key, a bad request, a quota wall).
fn retry_wait(error: &anyhow::Error, tries: usize) -> Option<Duration> {
    if let Some(refusal) = error.downcast_ref::<ProviderStatus>() {
        return match refusal.status.as_u16() {
            408 | 500 | 502 | 503 | 504 | 529 if tries < RETRY_LIMIT => {
                Some(backoff(tries, refusal.retry_after))
            }
            429 if tries < RATE_LIMIT_RETRIES
                && !is_hard_limit(&refusal.body)
                && refusal
                    .retry_after
                    .is_some_and(|seconds| seconds <= MAX_WAIT.as_secs()) =>
            {
                Some(backoff(tries, refusal.retry_after))
            }
            _ => None,
        };
    }
    let network = error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|error| error.is_timeout() || error.is_connect() || error.is_request())
    });
    (network && tries < RETRY_LIMIT).then(|| backoff(tries, None))
}

/// Waits, noticing a cancel within a tenth of a second.
fn pause(wait: Duration, cancel: &std::sync::atomic::AtomicBool) -> Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    let until = std::time::Instant::now() + wait;
    while std::time::Instant::now() < until {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::stream::Interrupted {
                partial: String::new(),
                reason: "cancelled".to_owned(),
            }
            .into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// Repeats a request that failed because the provider was briefly unavailable or the network
/// hiccuped. Errors that repeating cannot fix are returned at once.
fn with_retries(
    cancel: &std::sync::atomic::AtomicBool,
    attempt: impl Fn() -> Result<reqwest::blocking::Response>,
) -> Result<reqwest::blocking::Response> {
    let mut tries = 0;
    loop {
        match attempt() {
            Err(error) => match retry_wait(&error, tries) {
                Some(wait) => {
                    tries += 1;
                    pause(wait, cancel)?;
                }
                None => return Err(error),
            },
            done => return done,
        }
    }
}

fn openai_tool_specs(tools: ToolSet) -> Value {
    Value::Array(
        tools.definitions()
            .into_iter()
            .map(|tool| serde_json::json!({
                "type":"function",
                "function":{"name":tool.name, "description":tool.description, "parameters":tool.parameters}
            }))
            .collect(),
    )
}

/// How long an answer may be. Anthropic requires a limit; edits to real files need far more than
/// the 4096 that older models were capped at, but those older models refuse anything higher.
fn anthropic_max_tokens(model: &str) -> u32 {
    const OLDER_MODELS: u32 = 4_096;
    const RECENT_MODELS: u32 = 16_000;
    let name = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    // The first number in the name is the generation (claude-opus-4-5, claude-fable-5-1).
    let generation = name
        .split(['-', '.', '_'])
        .find_map(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    if name.starts_with("claude") && generation >= 4 {
        RECENT_MODELS
    } else {
        OLDER_MODELS
    }
}

/// Whether the server is Anthropic's own, which supports marking a prompt prefix for reuse.
/// Compatible servers are left alone, since an unknown field can make them refuse the request.
fn caches_prompts(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| host == "api.anthropic.com")
}

/// Asks Anthropic to remember the system prompt and the conversation so far, so the next round
/// pays the reduced cached price for them instead of the full one.
fn add_prompt_cache(body: &mut Value) {
    let marker = serde_json::json!({"type": "ephemeral"});
    if let Some(system) = body
        .get("system")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        body["system"] = serde_json::json!([
            {"type": "text", "text": system, "cache_control": marker}
        ]);
    }
    let Some(last) = body
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .and_then(|messages| messages.last_mut())
    else {
        return;
    };
    match last.get("content").cloned() {
        Some(Value::String(text)) if !text.is_empty() => {
            last["content"] = serde_json::json!([
                {"type": "text", "text": text, "cache_control": marker}
            ]);
        }
        Some(Value::Array(_)) => {
            if let Some(block) = last
                .get_mut("content")
                .and_then(Value::as_array_mut)
                .and_then(|blocks| blocks.last_mut())
                .and_then(Value::as_object_mut)
            {
                block.insert("cache_control".to_owned(), marker);
            }
        }
        _ => {}
    }
}

fn anthropic_tool_specs(tools: ToolSet) -> Value {
    Value::Array(
        tools.definitions()
            .into_iter()
            .map(|tool| {
                serde_json::json!({"name":tool.name, "description":tool.description, "input_schema":tool.parameters})
            })
            .collect(),
    )
}

fn google_tool_specs(tools: ToolSet) -> Value {
    Value::Array(vec![serde_json::json!({
        "functionDeclarations": tools.definitions()
            .into_iter()
            .map(|tool| serde_json::json!({"name":tool.name, "description":tool.description, "parameters":google_schema(&tool.parameters)}))
            .collect::<Vec<_>>()
    })])
}

fn google_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(object) => {
            let mut converted = serde_json::Map::new();
            for (key, value) in object {
                // Gemini accepts a restricted Schema object rather than arbitrary JSON Schema.
                if key == "additionalProperties" {
                    continue;
                }
                converted.insert(key.clone(), google_schema(value));
            }
            Value::Object(converted)
        }
        Value::Array(items) => Value::Array(items.iter().map(google_schema).collect()),
        _ => schema.clone(),
    }
}

fn openai_call_arguments(call: &Value) -> Result<Value> {
    let raw = call
        .pointer("/function/arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    serde_json::from_str(raw).context("parsing tool call arguments")
}

fn complete_anthropic(request: &Request) -> Result<AgentTurn> {
    let Request {
        account_id: _,
        effort: _,
        client,
        base_url,
        api_key,
        model,
        messages,
        tools,
        stream,
    } = *request;
    let system = messages
        .iter()
        .filter(|message| message.role == "system")
        .map(message_text)
        .collect::<Vec<_>>()
        .join("\n\n");
    let messages = messages
        .iter()
        .filter(|message| message.role != "system")
        .map(anthropic_message)
        .collect::<Result<Vec<_>>>()?;
    let mut body = serde_json::json!({ "model": model, "max_tokens": anthropic_max_tokens(model), "messages": messages, "stream": true });
    if !system.is_empty() {
        body["system"] = Value::String(system);
    }
    if tools.any() {
        body["tools"] = anthropic_tool_specs(tools);
    }
    let endpoint = format!("{}/messages", base_url.trim_end_matches('/'));
    if caches_prompts(base_url) {
        add_prompt_cache(&mut body);
    }
    let sent = add_effort(&mut body, request);
    let response = send_with_effort_fallback(body, sent, |body| {
        with_retries(stream.cancel, || {
            successful(
                client
                    .post(&endpoint)
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(body)
                    .send()
                    .context("sending request to the Anthropic Messages API")?,
            )
        })
    })?;
    parse_anthropic_stream(std::io::BufReader::new(response), stream)
}

fn anthropic_message(message: &ChatMessage) -> Result<Value> {
    if message.role == "assistant" {
        if let Some(calls) = &message.tool_calls {
            let mut content = Vec::new();
            if !message_text(message).is_empty() {
                content.push(serde_json::json!({"type":"text", "text":message_text(message)}));
            }
            content.extend(calls.iter().map(|call| {
                    Ok(serde_json::json!({
                        "type": "tool_use",
                        "id": call.get("id").and_then(Value::as_str).unwrap_or_default(),
                        "name": call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default(),
                        "input": openai_call_arguments(call)?,
                    }))
                })
                .collect::<Result<Vec<_>>>()?);
            return Ok(serde_json::json!({"role":"assistant", "content":content}));
        }
        return Ok(
            serde_json::json!({"role":"assistant", "content":anthropic_content(&message.content)?}),
        );
    }
    if message.role == "tool" {
        return Ok(serde_json::json!({
            "role":"user",
            "content":[{"type":"tool_result", "tool_use_id":message.tool_call_id, "content":message.content.as_str().unwrap_or_default()}]
        }));
    }
    Ok(serde_json::json!({"role":"user", "content":anthropic_content(&message.content)?}))
}

fn complete_google(request: &Request) -> Result<AgentTurn> {
    let Request {
        account_id: _,
        effort: _,
        client,
        base_url,
        api_key,
        model,
        messages,
        tools,
        stream,
    } = *request;
    // complete_turn enforces acknowledgement and redacts content before reaching this adapter.
    let system = messages
        .iter()
        .filter(|message| message.role == "system")
        .map(message_text)
        .collect::<Vec<_>>()
        .join("\n\n");
    let contents = messages
        .iter()
        .filter(|message| message.role != "system")
        .map(google_message)
        .collect::<Result<Vec<_>>>()?;
    let mut body = serde_json::json!({ "contents": contents });
    if !system.is_empty() {
        body["systemInstruction"] = serde_json::json!({ "parts": [{ "text": system }] });
    }
    if tools.any() {
        body["tools"] = google_tool_specs(tools);
    }
    let model = model.strip_prefix("models/").unwrap_or(model);
    let endpoint = format!(
        "{}/models/{model}:streamGenerateContent?alt=sse",
        base_url.trim_end_matches('/')
    );
    let sent = add_effort(&mut body, request);
    let response = send_with_effort_fallback(body, sent, |body| {
        send_google_request(client, &endpoint, api_key, body)
    })?;
    parse_google_stream(std::io::BufReader::new(response), stream)
}

fn send_google_request(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    body: &Value,
) -> Result<reqwest::blocking::Response> {
    const MAX_RETRIES: usize = 3;

    for attempt in 0..=MAX_RETRIES {
        let response = client
            .post(endpoint)
            .header("x-goog-api-key", api_key)
            .json(body)
            .send()
            .context("sending request to the Google Generative Language API")?;
        if response.status().as_u16() != 503 {
            return successful(response);
        }
        if attempt == MAX_RETRIES {
            return successful(response)
                .context("Gemini remained unavailable after 4 attempts (3 retries)");
        }
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok());
        thread::sleep(google_retry_delay(retry_after, attempt));
    }
    unreachable!("the bounded Gemini retry loop always returns")
}

fn google_retry_delay(retry_after: Option<&str>, attempt: usize) -> Duration {
    const MAX_DELAY: Duration = Duration::from_secs(10);
    retry_after
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .filter(|delay| !delay.is_zero())
        .unwrap_or_else(|| Duration::from_secs(1u64 << attempt.min(2)))
        .min(MAX_DELAY)
}

// Google's documented value for function calls whose original signature is unavailable,
// such as history produced by another model after a fallback switch.
const GOOGLE_SIGNATURE_PLACEHOLDER: &str = "skip_thought_signature_validator";

fn google_message(message: &ChatMessage) -> Result<Value> {
    if message.role == "assistant" {
        if let Some(calls) = &message.tool_calls {
            let mut parts = Vec::new();
            if !message_text(message).is_empty() {
                parts.extend(google_parts(&message.content)?);
            }
            parts.extend(calls.iter().map(|call| {
                    let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
                    let signature = message
                        .thought_signatures
                        .iter()
                        .find(|(call_id, _)| call_id == id)
                        .map_or(GOOGLE_SIGNATURE_PLACEHOLDER, |(_, signature)| signature.as_str());
                    Ok(serde_json::json!({
                        "thoughtSignature": signature,
                        "functionCall": {
                            "name": call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default(),
                            "args": openai_call_arguments(call)?,
                        }
                    }))
                })
                .collect::<Result<Vec<_>>>()?);
            return Ok(serde_json::json!({"role":"model", "parts":parts}));
        }
        return Ok(serde_json::json!({"role":"model", "parts":google_parts(&message.content)?}));
    }
    if message.role == "tool" {
        return Ok(serde_json::json!({
            "role":"user",
            "parts":[{"functionResponse":{"name":message.tool_name.as_deref().unwrap_or_default(), "response":{"content":message.content.as_str().unwrap_or_default()}}}]
        }));
    }
    Ok(serde_json::json!({"role":"user", "parts":google_parts(&message.content)?}))
}

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

fn anthropic_content(content: &Value) -> Result<Value> {
    let Some(parts) = content.as_array() else {
        return Ok(Value::String(
            content.as_str().unwrap_or_default().to_owned(),
        ));
    };
    let mut converted = Vec::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            converted.push(serde_json::json!({ "type": "text", "text": text }));
        } else if let Some(url) = part.pointer("/image_url/url").and_then(Value::as_str) {
            let (media_type, data) = parse_data_url(url)?;
            converted.push(serde_json::json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": data } }));
        }
    }
    Ok(Value::Array(converted))
}

fn google_parts(content: &Value) -> Result<Vec<Value>> {
    let Some(parts) = content.as_array() else {
        return Ok(vec![
            serde_json::json!({ "text": content.as_str().unwrap_or_default() }),
        ]);
    };
    let mut converted = Vec::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            converted.push(serde_json::json!({ "text": text }));
        } else if let Some(url) = part.pointer("/image_url/url").and_then(Value::as_str) {
            let (mime_type, data) = parse_data_url(url)?;
            converted
                .push(serde_json::json!({ "inlineData": { "mimeType": mime_type, "data": data } }));
        }
    }
    Ok(converted)
}

fn parse_data_url(url: &str) -> Result<(&str, &str)> {
    let (metadata, data) = url
        .split_once(',')
        .context("image attachment was not a data URL")?;
    let metadata = metadata
        .strip_prefix("data:")
        .context("image data URL has no MIME type")?;
    let mime_type = metadata
        .strip_suffix(";base64")
        .context("image data URL is not base64 encoded")?;
    Ok((mime_type, data))
}

#[cfg(test)]
fn extract_response(value: &Value) -> Result<String> {
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .context("provider response did not contain choices[0].message.content")?;

    if let Some(text) = content.as_str() {
        return Ok(text.to_owned());
    }
    if let Some(parts) = content.as_array() {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>();
        if !text.is_empty() {
            return Ok(text);
        }
    }
    bail!("provider returned an empty or unsupported message content format")
}

pub(crate) const PRIVACY_FAMILIES: [&str; 2] = ["Google/Gemini", "GLM/Z.ai"];

/// Whether `base_url` points at this computer.
fn is_loopback(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]"))
}

/// Adds the key as a bearer token, unless there is none (a local server).
fn with_bearer(
    builder: reqwest::blocking::RequestBuilder,
    key: &str,
) -> reqwest::blocking::RequestBuilder {
    if key.is_empty() {
        builder
    } else {
        builder.bearer_auth(key)
    }
}

fn privacy_risk(provider: &str, model: &str, base_url: &str) -> Option<&'static str> {
    let identity = format!("{provider} {model} {base_url}").to_ascii_lowercase();
    if ["gemini", "google", "generativelanguage", "googleapis.com"]
        .iter()
        .any(|marker| identity.contains(marker))
    {
        Some(PRIVACY_FAMILIES[0])
    } else if ["glm", "z.ai", "z ai"]
        .iter()
        .any(|marker| identity.contains(marker))
    {
        Some(PRIVACY_FAMILIES[1])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::Settings;
    use serde_json::{Value, json};

    use super::{
        ChatMessage, anthropic_content, anthropic_message, anthropic_tool_specs, extract_response,
        google_message, google_parts, google_retry_delay, google_tool_specs, is_usage_limit_error,
        openai_tool_specs, privacy_risk, redact_messages, restore_redactions,
        restore_redactions_value,
    };
    use crate::stream::Interrupted;

    #[test]
    fn only_addresses_on_this_computer_count_as_local() {
        for local in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
            "http://[::1]:1234/v1",
        ] {
            assert!(super::is_loopback(local), "{local}");
        }
        for remote in [
            "https://api.openai.com/v1",
            "http://localhost.evil.example/v1",
            "http://192.168.1.5:11434",
            "not a url",
        ] {
            assert!(!super::is_loopback(remote), "{remote}");
        }
        let client = reqwest::blocking::Client::new();
        let request = |key| {
            super::with_bearer(client.post("http://localhost/x"), key)
                .build()
                .unwrap()
        };
        assert!(request("").headers().get("authorization").is_none());
        assert!(request("k").headers().get("authorization").is_some());
    }

    #[test]
    fn extracts_plain_and_segmented_responses() {
        assert_eq!(
            extract_response(&json!({"choices":[{"message":{"content":"hello"}}]}))
                .expect("plain response"),
            "hello"
        );
        assert_eq!(
            extract_response(
                &json!({"choices":[{"message":{"content":[{"text":"hello "},{"text":"there"}]}}]})
            )
            .expect("segmented response"),
            "hello there"
        );
    }

    #[test]
    fn adapter_tools_match_the_provider_neutral_harness_registry() {
        let names = crate::tools::definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "list_files",
                "read_file",
                "search_text",
                "git_status",
                "git_diff",
                "git_log",
                "replace_text",
                "replace_in_file",
                "write_to_file",
                "create_file",
                "run_command",
                "request_plan_approval",
            ]
        );
        assert!(!names.iter().any(|name| name.contains("exec")));
        let openai_outside_plan = openai_tool_specs(crate::tools::ToolSet::Main {
            images: false,
            plan_mode: false,
            workflows: false,
        });
        assert!(openai_outside_plan.as_array().unwrap().iter().all(|tool| {
            tool.pointer("/function/name").and_then(Value::as_str) != Some("request_plan_approval")
        }));
        assert!(
            openai_tool_specs(crate::tools::ToolSet::Main {
                images: false,
                plan_mode: true,
                workflows: false
            })
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| {
                tool.pointer("/function/name").and_then(Value::as_str)
                    == Some("request_plan_approval")
            })
        );
        assert_eq!(
            anthropic_tool_specs(crate::tools::ToolSet::Main {
                images: false,
                plan_mode: true,
                workflows: false
            })
            .as_array()
            .unwrap()
            .len(),
            names.len()
        );
        assert_eq!(
            anthropic_tool_specs(crate::tools::ToolSet::Main {
                images: false,
                plan_mode: false,
                workflows: false
            })
            .as_array()
            .unwrap()
            .len(),
            names.len() - 1
        );
        assert_eq!(
            google_tool_specs(crate::tools::ToolSet::Main {
                images: false,
                plan_mode: true,
                workflows: false
            })[0]["functionDeclarations"]
                .as_array()
                .unwrap()
                .len(),
            names.len()
        );
        assert_eq!(
            google_tool_specs(crate::tools::ToolSet::Main {
                images: false,
                plan_mode: false,
                workflows: false
            })[0]["functionDeclarations"]
                .as_array()
                .unwrap()
                .len(),
            names.len() - 1
        );
        let google_specs = google_tool_specs(crate::tools::ToolSet::Main {
            images: false,
            plan_mode: true,
            workflows: false,
        });
        let google_json = google_specs.to_string();
        assert!(google_json.contains("\"type\":\"object\""));
        assert!(google_json.contains("\"type\":\"string\""));
        assert!(!google_json.contains("additionalProperties"));
    }

    #[test]
    fn gemini_503_retries_use_bounded_exponential_backoff() {
        assert_eq!(google_retry_delay(None, 0), Duration::from_secs(1));
        assert_eq!(google_retry_delay(None, 1), Duration::from_secs(2));
        assert_eq!(google_retry_delay(None, 2), Duration::from_secs(4));
        assert_eq!(google_retry_delay(Some("3"), 0), Duration::from_secs(3));
        assert_eq!(google_retry_delay(Some("60"), 0), Duration::from_secs(10));
        assert_eq!(
            google_retry_delay(Some("not-a-number"), 1),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn normalized_tools_translate_to_anthropic_and_google_protocols() {
        let call = ChatMessage::assistant_tool_calls(
            String::new(),
            vec![json!({
                "id":"call-7",
                "type":"function",
                "function":{"name":"read_file", "arguments":"{\"path\":\"src/main.rs\"}"}
            })],
        );
        let anthropic = anthropic_message(&call).expect("Anthropic tool-use block");
        assert_eq!(anthropic["role"], "assistant");
        assert_eq!(anthropic["content"][0]["type"], "tool_use");
        assert_eq!(anthropic["content"][0]["input"]["path"], "src/main.rs");

        let google = google_message(&call).expect("Gemini function call");
        assert_eq!(google["role"], "model");
        assert_eq!(
            google["parts"][0]["functionCall"]["args"]["path"],
            "src/main.rs"
        );

        let result = ChatMessage::tool_result(
            "call-7".to_owned(),
            "read_file".to_owned(),
            "file contents".to_owned(),
        );
        assert_eq!(
            anthropic_message(&result).unwrap()["content"][0]["tool_use_id"],
            "call-7"
        );
        assert_eq!(
            google_message(&result).unwrap()["parts"][0]["functionResponse"]["name"],
            "read_file"
        );
    }

    #[test]
    fn file_attachment_payload_does_not_replace_visible_prompt() {
        let message = ChatMessage::user_with_images(
            "look at @notes.md".to_owned(),
            "look at @notes.md\n\nfile contents".to_owned(),
            Vec::new(),
        );
        assert_eq!(message.display, "look at @notes.md");
        assert_eq!(message.content, json!("look at @notes.md\n\nfile contents"));
    }

    #[test]
    fn native_provider_payloads_translate_local_image_parts() {
        let message = json!([
            {"type":"text", "text":"inspect"},
            {"type":"image_url", "image_url":{"url":"data:image/png;base64,aGVsbG8="}}
        ]);
        let anthropic = anthropic_content(&message).expect("Anthropic blocks");
        assert_eq!(anthropic[1]["source"]["media_type"], "image/png");
        assert_eq!(anthropic[1]["source"]["data"], "aGVsbG8=");
        let google = google_parts(&message).expect("Google parts");
        assert_eq!(google[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(google[1]["inlineData"]["data"], "aGVsbG8=");
    }

    fn model_call(id: &str, name: &str) -> Value {
        json!({"id": id, "type": "function", "function": {"name": name, "arguments": "{}"}})
    }

    #[test]
    fn google_requests_return_the_thought_signature_on_function_calls() {
        let message = ChatMessage::assistant_tool_calls(
            String::new(),
            vec![
                model_call("c1", "list_files"),
                model_call("c2", "git_status"),
            ],
        )
        .with_thought_signatures(vec![("c1".to_owned(), "sig-abc".to_owned())]);
        let value = super::google_message(&message).expect("google message");
        let parts = value["parts"].as_array().expect("parts");
        assert_eq!(parts[0]["thoughtSignature"], "sig-abc");
        assert_eq!(parts[0]["functionCall"]["name"], "list_files");
        // A call from another model has no signature; Google accepts its documented placeholder.
        assert_eq!(
            parts[1]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
    }

    #[test]
    fn thought_signatures_never_reach_openai_style_request_bodies() {
        let message =
            ChatMessage::assistant_tool_calls(String::new(), vec![model_call("c1", "list_files")])
                .with_thought_signatures(vec![("c1".to_owned(), "sig-abc".to_owned())]);
        let body = serde_json::to_string(&message).expect("serialize");
        assert!(
            !body.contains("sig-abc") && !body.to_lowercase().contains("thought"),
            "{body}"
        );
    }

    #[test]
    fn fallback_never_follows_visible_partial_output() {
        let plain = anyhow::anyhow!("provider returned 429 Too Many Requests");
        assert!(super::should_fall_back(&plain));
        let early = anyhow::Error::new(Interrupted {
            partial: String::new(),
            reason: "provider error: quota exceeded".to_owned(),
        });
        assert!(super::should_fall_back(&early));
        let late = anyhow::Error::new(Interrupted {
            partial: "half an answer".to_owned(),
            reason: "provider error: quota exceeded".to_owned(),
        });
        assert!(!super::should_fall_back(&late));
        assert!(!super::should_fall_back(&anyhow::anyhow!(
            "invalid api key"
        )));
    }

    #[test]
    fn sensitive_values_are_redacted_locally_and_restorable() {
        let original = "Email me at person@example.com; key sk-abcdefghijklmnopqrstuvwxyz0123456789; codename frostbird";
        let message =
            ChatMessage::user_with_images(original.to_owned(), original.to_owned(), Vec::new());
        let (redacted, mapping) = redact_messages(&[message], &["frostbird".to_owned()]);
        let safe = redacted[0].content.as_str().expect("text content");
        assert!(!safe.contains("person@example.com"));
        assert!(!safe.contains("sk-abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(!safe.contains("frostbird"));
        assert_eq!(restore_redactions(safe.to_owned(), &mapping), original);

        let mut tool_message = ChatMessage::assistant_tool_calls(
            String::new(),
            vec![json!({
                "id":"call-1",
                "type":"function",
                "function":{"name":"read_file", "arguments":"{\"path\":\"frostbird/notes.md\"}"}
            })],
        );
        let (mut redacted_tool, tool_mapping) =
            redact_messages(&[tool_message.clone()], &["frostbird".to_owned()]);
        let sent_arguments = redacted_tool[0].tool_calls.as_ref().unwrap()[0]
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap();
        assert!(!sent_arguments.contains("frostbird"));
        tool_message.tool_calls = redacted_tool.pop().unwrap().tool_calls;
        let mut restored = tool_message.tool_calls.unwrap();
        for call in &mut restored {
            restore_redactions_value(call, &tool_mapping);
        }
        assert!(
            restored[0]["function"]["arguments"]
                .as_str()
                .unwrap()
                .contains("frostbird")
        );
    }

    #[test]
    fn usage_limit_errors_are_separated_from_other_provider_errors() {
        assert!(is_usage_limit_error(
            "provider returned 429: rate_limit_error"
        ));
        assert!(is_usage_limit_error("RESOURCE_EXHAUSTED: quota exceeded"));
        assert!(!is_usage_limit_error("invalid API key"));
    }

    #[test]
    fn gemini_requires_separate_acknowledgement_for_image_contents() {
        let mut settings = Settings::default();
        settings.provider = Some("google".to_owned());
        settings.model = Some("gemini-flash-latest".to_owned());
        let error =
            super::complete(&settings, &[]).expect_err("must require first-use acknowledgement");
        assert!(error.to_string().contains("acknowledge"));

        settings
            .privacy_acknowledged
            .push("Google/Gemini".to_owned());
        let image = ChatMessage::user_with_images(
            "look".to_owned(),
            "look".to_owned(),
            vec![
                json!({ "type":"image_url", "image_url":{ "url":"data:image/png;base64,aGVsbG8=" } }),
            ],
        );
        let error = super::complete(&settings, std::slice::from_ref(&image))
            .expect_err("image contents require separate consent");
        assert!(error.to_string().contains("image contents"));

        settings
            .privacy_image_acknowledged
            .push("Google/Gemini".to_owned());
        let error = super::complete(&settings, &[image])
            .expect_err("without credentials, the request should fail at key lookup");
        assert!(!error.to_string().contains("image contents"));
        assert!(error.to_string().contains("GOOGLE_API_KEY"));
    }

    #[test]
    fn flags_privacy_sensitive_model_and_endpoints_before_sending() {
        assert_eq!(
            privacy_risk(
                "openai-compatible",
                "gemini-3-flash",
                "https://example.test/v1"
            ),
            Some("Google/Gemini")
        );
        assert_eq!(
            privacy_risk("openai-compatible", "some-model", "https://api.z.ai/v1"),
            Some("GLM/Z.ai")
        );
        assert_eq!(
            privacy_risk("groq", "some-groq-model", "https://api.groq.com/openai/v1"),
            None
        );
    }

    use crate::testutil::serve;

    const OK_STREAM: &str =
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";

    fn run_against(base: &str, model: &str, effort: crate::Effort) -> anyhow::Result<()> {
        let client = reqwest::blocking::Client::new();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let on_event = |_event: crate::stream::StreamEvent| {};
        let stream = crate::stream::Stream {
            on_event: &on_event,
            cancel: &cancel,
        };
        let messages = [super::ChatMessage::user_with_images(
            "hi".to_owned(),
            "hi".to_owned(),
            Vec::new(),
        )];
        let request = super::Request {
            effort: Some(super::EffortRequest {
                api: crate::effort_support::Api::OpenAiCompatible,
                wanted: effort,
            }),
            client: &client,
            base_url: base,
            api_key: "test-key",
            model,
            messages: &messages,
            tools: crate::tools::ToolSet::None,
            stream: &stream,
            account_id: None,
        };
        super::complete_openai_compatible(&request).map(|_| ())
    }

    #[test]
    fn effort_is_sent_to_a_model_that_has_it() {
        let (base, seen) = serve(vec![(200, "text/event-stream", OK_STREAM)]);
        run_against(&base, "gpt-6-fallback-sent", crate::Effort::High).expect("ok");
        let bodies = seen.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let sent: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(sent["reasoning_effort"], "high");
    }

    #[test]
    fn a_model_without_effort_levels_is_sent_no_parameter() {
        let (base, seen) = serve(vec![(200, "text/event-stream", OK_STREAM)]);
        run_against(&base, "deepseek-reasoner", crate::Effort::Max).expect("ok");
        let sent: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
        assert!(sent.get("reasoning_effort").is_none(), "{sent}");
    }

    #[test]
    fn deepseek_and_kimi_k3_are_sent_reasoning_effort_max() {
        for model in ["deepseek-flash", "kimi-k3"] {
            let (base, seen) = serve(vec![(200, "text/event-stream", OK_STREAM)]);
            run_against(&base, model, crate::Effort::Ultimate).expect("ok");
            let sent: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
            assert_eq!(sent["reasoning_effort"], "max", "{model}: {sent}");
            assert!(sent.get("thinking").is_none(), "{model}: {sent}");
        }
    }

    #[test]
    fn a_deepseek_endpoint_that_rejects_the_effort_still_gets_its_answer() {
        let (base, seen) = serve(vec![
            (
                400,
                "application/json",
                "{\"error\":{\"message\":\"unknown field reasoning_effort\"}}",
            ),
            (200, "text/event-stream", OK_STREAM),
        ]);
        let model = "deepseek-v4-flash-vision-exp";
        run_against(&base, model, crate::Effort::High).expect("the retry succeeds");
        let bodies = seen.lock().unwrap();
        assert!(bodies[0].contains("reasoning_effort"));
        assert!(!bodies[1].contains("reasoning_effort"), "{}", bodies[1]);
    }

    #[test]
    fn a_provider_that_rejects_the_effort_parameter_gets_one_retry_without_it() {
        let (base, seen) = serve(vec![
            (
                400,
                "application/json",
                "{\"error\":{\"message\":\"Unrecognized request argument supplied: reasoning_effort\"}}",
            ),
            (200, "text/event-stream", OK_STREAM),
        ]);
        let model = "gpt-6-fallback-rejects";
        run_against(&base, model, crate::Effort::High).expect("the retry succeeds");
        let bodies = seen.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].contains("reasoning_effort"));
        assert!(!bodies[1].contains("reasoning_effort"), "{}", bodies[1]);
        assert!(crate::effort_support::rejected(
            crate::effort_support::Api::OpenAiCompatible,
            model
        ));
        drop(bodies);
        // From now on the model is not even asked.
        let (base, seen) = serve(vec![(200, "text/event-stream", OK_STREAM)]);
        run_against(&base, model, crate::Effort::High).expect("ok");
        assert!(!seen.lock().unwrap()[0].contains("reasoning_effort"));
    }

    const CHATGPT_STREAM: &str = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi there\"}\n\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"list_files\",\"arguments\":\"{}\"}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}}\n\n";

    fn run_chatgpt(
        base: &str,
        model: &str,
        effort: crate::Effort,
        account: Option<&str>,
    ) -> anyhow::Result<super::AgentTurn> {
        let client = reqwest::blocking::Client::new();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let on_event = |_event: crate::stream::StreamEvent| {};
        let stream = crate::stream::Stream {
            on_event: &on_event,
            cancel: &cancel,
        };
        let messages = [
            super::ChatMessage::system("be brief".to_owned()),
            super::ChatMessage::user_with_images("hi".to_owned(), "hi".to_owned(), Vec::new()),
        ];
        let request = super::Request {
            effort: Some(super::EffortRequest {
                api: crate::effort_support::Api::ChatGpt,
                wanted: effort,
            }),
            client: &client,
            base_url: base,
            api_key: "access-token-123",
            model,
            messages: &messages,
            tools: crate::tools::ToolSet::Explore,
            stream: &stream,
            account_id: account,
        };
        super::complete_chatgpt(&request)
    }

    #[test]
    fn the_chatgpt_adapter_sends_the_account_headers_and_a_responses_body() {
        let (base, seen) =
            crate::testutil::serve_full(vec![(200, "text/event-stream", CHATGPT_STREAM)]);
        let turn = run_chatgpt(
            &format!("{base}/backend-api/codex"),
            "gpt-6-chatgpt-test",
            crate::Effort::High,
            Some("acct-77"),
        )
        .expect("turn");
        assert_eq!(turn.text, "Hi there");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "list_files");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let (head, body) = &requests[0];
        let lower = head.to_ascii_lowercase();
        assert!(
            head.starts_with("POST /backend-api/codex/responses "),
            "{head}"
        );
        assert!(
            lower.contains("authorization: bearer access-token-123"),
            "{head}"
        );
        assert!(lower.contains("chatgpt-account-id: acct-77"), "{head}");
        assert!(
            lower.contains("openai-beta: responses=experimental"),
            "{head}"
        );
        assert!(lower.contains("originator: codex_cli_rs"), "{head}");
        assert!(lower.contains("accept: text/event-stream"), "{head}");
        let sent: Value = serde_json::from_str(body).unwrap();
        assert_eq!(sent["instructions"], "be brief");
        assert_eq!(sent["input"][0]["content"][0]["text"], "hi");
        assert_eq!(sent["reasoning"]["effort"], "high");
        assert_eq!(sent["store"], false);
        assert!(
            sent["tools"]
                .as_array()
                .is_some_and(|tools| !tools.is_empty())
        );
        assert!(
            sent.get("messages").is_none(),
            "not the chat-completions shape"
        );
    }

    #[test]
    fn the_chatgpt_adapter_omits_the_account_header_when_unknown_and_retries_without_rejected_effort()
     {
        let (base, seen) = crate::testutil::serve_full(vec![
            (
                400,
                "application/json",
                "{\"error\":{\"message\":\"Unsupported parameter: reasoning.effort\"}}",
            ),
            (200, "text/event-stream", CHATGPT_STREAM),
        ]);
        let turn = run_chatgpt(&base, "gpt-6-chatgpt-rejects", crate::Effort::Medium, None)
            .expect("retried");
        assert_eq!(turn.text, "Hi there");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            !requests[0]
                .0
                .to_ascii_lowercase()
                .contains("chatgpt-account-id")
        );
        assert!(requests[0].1.contains("\"reasoning\""));
        assert!(
            !requests[1].1.contains("\"reasoning\""),
            "{}",
            requests[1].1
        );
    }

    #[test]
    fn recent_claude_models_may_write_long_answers_and_older_ones_keep_their_old_cap() {
        for recent in [
            "claude-opus-5-5",
            "claude-fable-5-1",
            "anthropic/claude-sonnet-4-5",
            "claude-opus-4",
        ] {
            assert_eq!(super::anthropic_max_tokens(recent), 16_000, "{recent}");
        }
        for older in [
            "claude-3-haiku-20240307",
            "claude-3-5-sonnet",
            "mystery-model",
            "kimi-k3",
        ] {
            assert_eq!(super::anthropic_max_tokens(older), 4_096, "{older}");
        }
    }

    #[test]
    fn only_anthropics_own_servers_are_asked_to_cache_the_prompt() {
        assert!(super::caches_prompts("https://api.anthropic.com/v1"));
        for other in [
            "https://api.kimi.com/coding/",
            "https://api.anthropic.com.evil.example/v1",
            "http://localhost:8080",
            "not a url",
        ] {
            assert!(!super::caches_prompts(other), "{other}");
        }
    }

    #[test]
    fn the_system_prompt_and_the_latest_message_are_marked_for_caching() {
        let mut body = serde_json::json!({
            "system": "be brief",
            "messages": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": [{"type": "text", "text": "a"}]},
                {"role": "user", "content": "latest"}
            ]
        });
        super::add_prompt_cache(&mut body);
        assert_eq!(body["system"][0]["text"], "be brief");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["messages"][2]["content"][0]["text"], "latest");
        assert_eq!(
            body["messages"][2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            body["messages"][0]["content"], "first",
            "older messages are untouched"
        );
        // A message that already has blocks gets the mark on its last block.
        let mut blocks = serde_json::json!({
            "messages": [{"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t", "content": "x"},
                {"type": "text", "text": "and then"}
            ]}]
        });
        super::add_prompt_cache(&mut blocks);
        assert!(
            blocks["messages"][0]["content"][0]
                .get("cache_control")
                .is_none()
        );
        assert_eq!(
            blocks["messages"][0]["content"][1]["cache_control"]["type"],
            "ephemeral"
        );
        // Nothing to mark is not an error.
        let mut empty = serde_json::json!({"messages": []});
        super::add_prompt_cache(&mut empty);
    }

    fn refusal(status: u16, body: &str, retry_after: Option<u64>) -> anyhow::Error {
        anyhow::Error::new(super::ProviderStatus {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            body: body.to_owned(),
            retry_after,
        })
    }

    fn seconds(wait: Option<std::time::Duration>) -> Option<u64> {
        wait.map(|wait| wait.as_secs())
    }

    #[test]
    fn a_briefly_unavailable_provider_is_retried_with_growing_waits_and_then_given_up_on() {
        for status in [500, 502, 503, 504, 529, 408] {
            let error = refusal(status, "overloaded", None);
            assert_eq!(seconds(super::retry_wait(&error, 0)), Some(1), "{status}");
            assert_eq!(seconds(super::retry_wait(&error, 1)), Some(2));
            assert_eq!(seconds(super::retry_wait(&error, 2)), Some(4));
            assert_eq!(super::retry_wait(&error, 3), None, "three retries at most");
        }
        let asked = refusal(503, "busy", Some(7));
        assert_eq!(
            seconds(super::retry_wait(&asked, 0)),
            Some(7),
            "what the provider asked for"
        );
        let too_long = refusal(503, "busy", Some(600));
        assert_eq!(
            seconds(super::retry_wait(&too_long, 0)),
            Some(30),
            "never longer than half a minute"
        );
    }

    #[test]
    fn mistakes_and_walls_are_not_retried() {
        for status in [400, 401, 403, 404, 422] {
            assert_eq!(
                super::retry_wait(&refusal(status, "no", None), 0),
                None,
                "{status}"
            );
        }
        assert_eq!(
            super::retry_wait(&anyhow::anyhow!("something else"), 0),
            None
        );
    }

    #[test]
    fn a_rate_limit_is_retried_only_when_it_names_a_short_wait_and_is_not_a_quota() {
        let brief = refusal(429, "slow down", Some(3));
        assert_eq!(seconds(super::retry_wait(&brief, 0)), Some(3));
        assert_eq!(seconds(super::retry_wait(&brief, 1)), Some(3));
        assert_eq!(super::retry_wait(&brief, 2), None, "two retries at most");
        assert_eq!(
            super::retry_wait(&refusal(429, "slow down", None), 0),
            None,
            "no stated wait"
        );
        assert_eq!(
            super::retry_wait(&refusal(429, "slow down", Some(300)), 0),
            None,
            "too long to wait"
        );
        for wall in [
            "You exceeded your current quota",
            "usage limit reached",
            "insufficient credits",
            "billing hard limit",
        ] {
            assert_eq!(
                super::retry_wait(&refusal(429, wall, Some(3)), 0),
                None,
                "{wall}"
            );
        }
    }

    #[test]
    fn a_request_that_fails_with_a_server_error_succeeds_on_a_later_attempt() {
        let (base, seen) = crate::testutil::serve_full(vec![
            (503, "application/json", "{\"error\":\"overloaded\"}"),
            (502, "text/plain", "bad gateway"),
            (200, "text/event-stream", CHATGPT_STREAM),
        ]);
        let turn =
            run_chatgpt(&base, "gpt-6-chatgpt-retry", crate::Effort::Low, None).expect("turn");
        assert_eq!(turn.text, "Hi there");
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_provider_that_stays_down_is_given_up_on_after_the_retries() {
        let (base, seen) = crate::testutil::serve_full(vec![
            (500, "text/plain", "down"),
            (500, "text/plain", "down"),
            (500, "text/plain", "down"),
            (500, "text/plain", "down"),
            (200, "text/event-stream", CHATGPT_STREAM),
        ]);
        let error = run_chatgpt(&base, "gpt-6-chatgpt-down", crate::Effort::Low, None).unwrap_err();
        assert!(format!("{error:#}").contains("500"), "{error:#}");
        assert_eq!(
            seen.lock().unwrap().len(),
            4,
            "one attempt and three retries"
        );
    }

    #[test]
    fn a_rejected_key_is_reported_straight_away() {
        let (base, seen) = crate::testutil::serve_full(vec![
            (401, "application/json", "{\"error\":\"bad key\"}"),
            (200, "text/event-stream", CHATGPT_STREAM),
        ]);
        let error = run_chatgpt(&base, "gpt-6-chatgpt-key", crate::Effort::Low, None).unwrap_err();
        assert!(format!("{error:#}").contains("401"));
        assert_eq!(seen.lock().unwrap().len(), 1, "no retry");
    }

    #[test]
    fn a_connection_that_cannot_be_made_is_retried_then_reported() {
        // Nothing listens on port 1.
        let error = run_chatgpt(
            "http://127.0.0.1:1",
            "gpt-6-chatgpt-offline",
            crate::Effort::Low,
            None,
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("sending request"),
            "{error:#}"
        );
    }

    #[test]
    fn a_refused_chatgpt_request_reports_the_status() {
        let (base, _) =
            crate::testutil::serve_full(vec![(401, "application/json", "{\"error\":\"expired\"}")]);
        let error = run_chatgpt(&base, "gpt-6-chatgpt-401", crate::Effort::Low, None).unwrap_err();
        assert!(format!("{error:#}").contains("401"), "{error:#}");
    }

    #[test]
    fn an_unrelated_400_is_reported_and_not_retried() {
        let (base, seen) = serve(vec![
            (
                400,
                "application/json",
                "{\"error\":\"messages must not be empty\"}",
            ),
            (200, "text/event-stream", OK_STREAM),
        ]);
        let error = run_against(&base, "gpt-6-fallback-unrelated", crate::Effort::High)
            .expect_err("a real error");
        assert!(
            format!("{error:#}").contains("messages must not be empty"),
            "{error:#}"
        );
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "no retry for unrelated errors"
        );
    }
}
