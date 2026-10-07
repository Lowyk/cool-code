use anyhow::{Context, Result, anyhow, bail};
use reqwest::Url;
use serde_json::Value;

use crate::{ModelInfo, ModelProfile, ProviderProfile};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FetchedModel {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) free: Option<bool>,
    pub(crate) tools: Option<bool>,
    pub(crate) context: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LimitLine {
    pub(crate) label: String,
    pub(crate) value: String,
    /// Fraction remaining (0.0 to 1.0) when the line can be drawn as a bar.
    pub(crate) remaining: Option<f32>,
    /// What is left, in tokens, on a pay-as-you-go balance line.
    pub(crate) balance_tokens: Option<u64>,
}

/// Resolves an optional endpoint setting against the provider's base URL.
///
/// Accepts a path relative to the base URL (`models`), a host-absolute path (`/v1/models`), or a
/// full URL. The result must use the provider's own host and port, and be HTTPS (plain HTTP only
/// for loopback addresses), because requests to it carry the API key.
pub(crate) fn resolve_endpoint(base_url: &str, input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("the endpoint is empty");
    }
    let base =
        Url::parse(base_url.trim()).map_err(|_| anyhow!("the base URL is not a valid URL"))?;
    let url = if input.contains("://") {
        Url::parse(input).map_err(|_| anyhow!("`{input}` is not a valid URL"))?
    } else if input.starts_with('/') {
        base.join(input)
            .map_err(|_| anyhow!("`{input}` is not a valid path"))?
    } else {
        let mut directory = base.clone();
        if !directory.path().ends_with('/') {
            let path = format!("{}/", directory.path());
            directory.set_path(&path);
        }
        directory
            .join(input)
            .map_err(|_| anyhow!("`{input}` is not a valid path"))?
    };
    if !url.username().is_empty() || url.password().is_some() {
        bail!("the endpoint must not contain a username or password");
    }
    let loopback = url.host_str().is_some_and(|host| {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() != base.scheme() {
        bail!("the endpoint must use the same scheme as the base URL");
    }
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => bail!("the endpoint must use https (http is allowed only for localhost)"),
        other => bail!("the endpoint scheme `{other}` is not supported"),
    }
    if url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
    {
        bail!(
            "the endpoint must be on the same host as the base URL, because it receives the API key"
        );
    }
    Ok(url.to_string())
}

/// Describes the shape of a saved API key without revealing any of it: its length and anything
/// that commonly makes providers reject a pasted key.
pub(crate) fn describe_key(key: &str, expected_prefix: Option<&str>) -> String {
    if key.is_empty() {
        return "empty".to_owned();
    }
    let mut problems = Vec::new();
    let quote = |c: char| matches!(c, '"' | '\'' | '`');
    if key.starts_with(quote) || key.ends_with(quote) {
        problems.push("wrapped in quotes".to_owned());
    }
    if key.chars().any(char::is_whitespace) {
        problems.push("contains spaces or line breaks".to_owned());
    }
    if key.len() >= 7 && key[..7].eq_ignore_ascii_case("bearer ") {
        problems.push("starts with Bearer".to_owned());
    }
    if let Some(prefix) = expected_prefix
        && !key.starts_with(prefix)
    {
        problems.push(format!("does not start with {prefix}"));
    }
    let count = key.chars().count();
    if problems.is_empty() {
        format!("{count} characters · looks well-formed")
    } else {
        format!("{count} characters · {}", problems.join(", "))
    }
}

/// Explains a rejected API key (HTTP 401 or 403) in terms of what the user can do about it;
/// other errors pass through unchanged.
pub(crate) fn friendly_fetch_error(raw: &str) -> String {
    let rejected = ["provider returned 401", "provider returned 403"]
        .iter()
        .any(|marker| raw.contains(marker));
    if rejected {
        "the provider rejected the API key. Select this provider and press e to enter a new key."
            .to_owned()
    } else {
        raw.to_owned()
    }
}

/// Replaces control characters (escape sequences and line breaks) with spaces so
/// text from a provider cannot move the cursor or inject lines into the terminal.
pub(crate) fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Reads a models list in the common shapes: `{"data": [...]}`, `{"models": [...]}`, or a bare
/// array of objects or strings. Entries that declare a non-text `model_kind` are skipped.
pub(crate) fn parse_models(value: &Value) -> Vec<FetchedModel> {
    let entries: &[Value] = match value {
        Value::Array(entries) => entries,
        Value::Object(object) => object
            .get("data")
            .or_else(|| object.get("models"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default(),
        _ => &[],
    };
    let mut models: Vec<FetchedModel> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let (id, object) = match entry {
            Value::String(id) => (id.as_str(), None),
            Value::Object(object) => {
                let Some(id) = object
                    .get("id")
                    .or_else(|| object.get("name"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                (id, Some(object))
            }
            _ => continue,
        };
        let id = clean(id);
        let id = id.as_str();
        if id.is_empty() || !seen.insert(id.to_owned()) {
            continue;
        }
        let field = |name: &str| object.and_then(|object| object.get(name));
        if field("model_kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "text")
        {
            continue;
        }
        models.push(FetchedModel {
            id: id.to_owned(),
            name: field("name")
                .and_then(Value::as_str)
                .map(clean)
                .filter(|name| !name.is_empty() && name != id)
                .unwrap_or_default(),
            free: field("billing_free").and_then(Value::as_bool),
            tools: field("supports_tools").and_then(Value::as_bool),
            context: field("context_window")
                .or_else(|| field("context_length"))
                .and_then(Value::as_u64),
        });
    }
    models
}

/// Adds models the provider does not list yet and refreshes metadata; existing names are kept.
/// Returns how many models were added.
pub(crate) fn merge_models(profile: &mut ProviderProfile, fetched: &[FetchedModel]) -> usize {
    // A provider with only a single `model` and no list treats that model as its list.
    if profile.models.is_empty() && !profile.model.is_empty() {
        profile.models.push(ModelProfile {
            id: profile.model.clone(),
            name: String::new(),
        });
    }
    let mut added = 0;
    let mut known: std::collections::HashSet<String> = profile
        .models
        .iter()
        .map(|existing| existing.id.to_ascii_lowercase())
        .collect();
    for model in fetched {
        if known.insert(model.id.to_ascii_lowercase()) {
            profile.models.push(ModelProfile {
                id: model.id.clone(),
                name: model.name.clone(),
            });
            added += 1;
        }
        let info = ModelInfo {
            free: model.free,
            tools: model.tools,
            context: model.context,
        };
        if info != ModelInfo::default() {
            profile.model_info.insert(model.id.clone(), info);
        }
    }
    if profile.model.is_empty()
        && let Some(first) = profile.models.first()
    {
        profile.model = first.id.clone();
    }
    added
}

/// Short badges for a model list row, such as " · free · no tools · 262k ctx" (empty when
/// nothing is known).
pub(crate) fn model_tags(info: Option<&ModelInfo>) -> String {
    let Some(info) = info else {
        return String::new();
    };
    let mut tags = Vec::new();
    if info.free == Some(true) {
        tags.push("free".to_owned());
    }
    if info.tools == Some(false) {
        tags.push("no tools".to_owned());
    }
    if let Some(context) = info.context {
        let compact = if context >= 1_000_000 {
            format!("{}M", context / 1_000_000)
        } else if context >= 1_000 {
            format!("{}k", context / 1_000)
        } else {
            context.to_string()
        };
        tags.push(format!("{compact} ctx"));
    }
    if tags.is_empty() {
        String::new()
    } else {
        format!(" · {}", tags.join(" · "))
    }
}

pub(crate) fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn short_time(raw: &str) -> String {
    match (raw.get(..10), raw.get(10..11), raw.get(11..16)) {
        (Some(date), Some("T"), Some(time)) => format!("{date} {time}"),
        _ => raw.to_owned(),
    }
}

fn tokens(value: &Value) -> Option<String> {
    value
        .as_u64()
        .map(|n| format!("{} tokens", group_digits(n)))
}

/// Turns a limits response into display lines. The MultiAI shape (account balance, subscription
/// windows, per-key limit) gets a dedicated summary; anything else lists its scalar fields.
pub(crate) fn summarize_limits(value: &Value) -> Vec<LimitLine> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let line = |label: &str, value: String, remaining: Option<f32>| LimitLine {
        label: label.to_owned(),
        value,
        remaining,
        balance_tokens: None,
    };
    if !["account_balance", "subscription", "api_key"]
        .iter()
        .any(|key| object.contains_key(*key))
    {
        let mut lines = Vec::new();
        flatten_scalars("", value, &mut lines);
        lines.truncate(14);
        return lines
            .into_iter()
            .map(|(path, text)| line(&clean(&path), text, None))
            .collect();
    }
    let mut lines = Vec::new();
    if let Some(balance) = object
        .get("account_balance")
        .and_then(|balance| balance.get("tokens"))
        .and_then(tokens)
    {
        lines.push(LimitLine {
            balance_tokens: object
                .get("account_balance")
                .and_then(|balance| balance.get("tokens"))
                .and_then(Value::as_u64),
            ..line("Balance", balance, None)
        });
    }
    if let Some(subscription) = object.get("subscription") {
        let active = subscription
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        lines.push(line(
            "Plan",
            if active {
                "active"
            } else {
                "no active subscription"
            }
            .to_owned(),
            None,
        ));
        for (key, label) in [
            ("five_hour", "5-hour"),
            ("weekly", "Weekly"),
            ("monthly", "Monthly"),
        ] {
            let Some(window) = subscription.pointer(&format!("/limits/{key}")) else {
                continue;
            };
            if window.get("enabled").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            let mut parts = Vec::new();
            if let (Some(used), Some(limit)) = (
                window.get("used_tokens").and_then(Value::as_u64),
                window.get("limit_tokens").and_then(Value::as_u64),
            ) {
                parts.push(format!(
                    "{} / {} tokens",
                    group_digits(used),
                    group_digits(limit)
                ));
            }
            let remaining = window
                .get("remaining_percent")
                .and_then(Value::as_f64)
                .map(|percent| (percent / 100.0).clamp(0.0, 1.0) as f32);
            if let Some(percent) = window.get("remaining_percent").and_then(Value::as_f64) {
                parts.push(format!("{percent:.0}% left"));
            }
            if let Some(reset) = window.as_object().and_then(|window| {
                window
                    .iter()
                    .find(|(name, value)| name.contains("reset") && value.is_string())
                    .and_then(|(_, value)| value.as_str())
            }) {
                parts.push(format!("resets {}", short_time(reset)));
            }
            lines.push(line(label, parts.join(" · "), remaining));
        }
    }
    if let Some(key) = object.get("api_key") {
        let spent = key.get("tokens_spent").and_then(Value::as_u64).unwrap_or(0);
        let limit = key.get("token_limit").and_then(Value::as_u64);
        let text = match (key.get("unlimited").and_then(Value::as_bool), limit) {
            (_, Some(limit)) => {
                let left = key
                    .get("limit_remaining")
                    .and_then(Value::as_u64)
                    .map(|left| format!(" · {} left", group_digits(left)))
                    .unwrap_or_default();
                format!(
                    "{} / {} tokens{left}",
                    group_digits(spent),
                    group_digits(limit)
                )
            }
            (Some(true), None) => format!("no limit · spent {} tokens", group_digits(spent)),
            _ => format!("spent {} tokens", group_digits(spent)),
        };
        lines.push(line("This key", text, None));
    }
    lines
}

/// Generic limit values come from an arbitrary endpoint, so they are kept short.
fn capped(text: &str) -> String {
    const MAX: usize = 80;
    if text.chars().count() <= MAX {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(MAX - 1).collect::<String>())
    }
}

fn flatten_scalars(path: &str, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                flatten_scalars(&child_path, child, out);
            }
        }
        Value::String(text) => out.push((path.to_owned(), capped(&clean(text)))),
        Value::Number(number) => out.push((path.to_owned(), number.to_string())),
        Value::Bool(flag) => {
            out.push((path.to_owned(), if *flag { "yes" } else { "no" }.to_owned()))
        }
        _ => {}
    }
}

/// GETs `url` with the API key as a bearer token. Redirects are refused so the key can never be
/// forwarded to another host.
// A models catalog is a few hundred KB; anything far larger is not a legitimate response.
#[cfg(not(test))]
const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
#[cfg(test)]
const MAX_BODY_BYTES: u64 = 64 * 1024;

pub(crate) fn fetch_json(url: &str, api_key: &str) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("creating HTTP client")?;
    let response = client
        .get(url)
        .bearer_auth(api_key)
        .header("Accept", "application/json")
        .send()
        // reqwest errors print the request URL, which could carry a secret in its query.
        .map_err(reqwest::Error::without_url)
        .context("sending request to the provider")?;
    let status = response.status();
    if status.is_redirection() {
        bail!("the provider redirected the request; update the endpoint to its final address");
    }
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| anyhow!("reading the provider response: {error}"))?;
    if bytes.len() as u64 > MAX_BODY_BYTES {
        bail!("the provider response is too large");
    }
    let body = String::from_utf8_lossy(&bytes);
    if !status.is_success() {
        let shown: String = clean(&body.replace(api_key, "<key>"))
            .chars()
            .take(300)
            .collect();
        bail!("provider returned {status}: {shown}");
    }
    serde_json::from_str(&body).context("the provider did not return JSON")
}

#[cfg(test)]
mod tests {
    use super::{
        FetchedModel, group_digits, merge_models, parse_models, resolve_endpoint, summarize_limits,
    };
    use crate::{ModelProfile, ProviderProfile};
    use serde_json::{Value, json};

    const BASE: &str = "https://multiai.store/v1";

    #[test]
    fn relative_paths_resolve_against_the_base_url() {
        assert_eq!(
            resolve_endpoint(BASE, "models").unwrap(),
            "https://multiai.store/v1/models"
        );
        assert_eq!(
            resolve_endpoint("https://multiai.store/v1/", "subscription/limits").unwrap(),
            "https://multiai.store/v1/subscription/limits"
        );
        assert_eq!(
            resolve_endpoint(BASE, "/v1/models").unwrap(),
            "https://multiai.store/v1/models"
        );
    }

    #[test]
    fn full_urls_must_stay_on_the_providers_host() {
        assert!(resolve_endpoint(BASE, "https://multiai.store/v1/models").is_ok());
        assert!(resolve_endpoint(BASE, "https://MULTIAI.store/other").is_ok());
        for bad in [
            "https://evil.example/v1/models",
            "https://multiai.store.evil.example/v1/models",
            "https://multiai.store:8443/v1/models",
            "https://user:pass@multiai.store/v1/models",
            "http://multiai.store/v1/models",
            "ftp://multiai.store/models",
            "https://",
            "",
            "   ",
        ] {
            assert!(
                resolve_endpoint(BASE, bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn plain_http_is_allowed_only_for_local_servers() {
        assert!(resolve_endpoint("http://localhost:8080/v1", "models").is_ok());
        assert!(resolve_endpoint("http://127.0.0.1:11434/v1", "models").is_ok());
        assert!(resolve_endpoint("http://example.com/v1", "models").is_err());
    }

    fn real_models_fixture() -> Value {
        json!({
            "catalog_revision": "9ebcf2781e1f51da5001",
            "data": [
                {"id": "amazon/nova-lite-v1", "name": "Amazon: Nova Lite 1.0", "model_kind": "text",
                 "billing_free": false, "supports_tools": true, "context_window": 300000},
                {"id": "deepseek/deepseek-v4-flash-free", "name": "DeepSeek V4 Flash (free)",
                 "model_kind": "text", "billing_free": true, "supports_tools": true,
                 "context_window": 1048576},
                {"id": "liquid/no-tools", "name": "No Tools", "model_kind": "text",
                 "supports_tools": false},
                {"id": "stability/picture", "name": "Picture", "model_kind": "image"},
                {"id": "openai/whisper", "model_kind": "audio_transcription"},
                {"id": "plain/unlabeled"}
            ],
            "object": "list"
        })
    }

    #[test]
    fn models_parse_from_the_real_catalog_shape_keeping_text_models_only() {
        let models = parse_models(&real_models_fixture());
        let ids = models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "amazon/nova-lite-v1",
                "deepseek/deepseek-v4-flash-free",
                "liquid/no-tools",
                "plain/unlabeled"
            ]
        );
        let free = &models[1];
        assert_eq!(free.name, "DeepSeek V4 Flash (free)");
        assert_eq!(free.free, Some(true));
        assert_eq!(free.tools, Some(true));
        assert_eq!(free.context, Some(1_048_576));
        assert_eq!(models[2].tools, Some(false));
        assert_eq!(models[3].free, None);
    }

    #[test]
    fn models_parse_from_other_common_shapes() {
        let wrapped = parse_models(&json!({"models": [{"id": "a"}, {"id": "b"}]}));
        assert_eq!(wrapped.len(), 2);
        let bare = parse_models(&json!([{"id": "a"}, "b", {"id": "a"}]));
        assert_eq!(
            bare.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(parse_models(&json!({"error": "nope"})).is_empty());
        assert!(parse_models(&json!("text")).is_empty());
    }

    #[test]
    fn merging_adds_new_models_and_keeps_existing_names() {
        let mut profile = ProviderProfile {
            id: "p".to_owned(),
            name: "MultiAI".to_owned(),
            models: vec![ModelProfile {
                id: "amazon/nova-lite-v1".to_owned(),
                name: "My Nova".to_owned(),
            }],
            ..Default::default()
        };
        let fetched = parse_models(&real_models_fixture());
        let added = merge_models(&mut profile, &fetched);
        assert_eq!(added, 3);
        assert_eq!(profile.models.len(), 4);
        assert_eq!(profile.models[0].name, "My Nova");
        assert_eq!(profile.models[1].id, "deepseek/deepseek-v4-flash-free");
        assert_eq!(profile.model, "amazon/nova-lite-v1");
        assert_eq!(
            profile.model_info["deepseek/deepseek-v4-flash-free"].free,
            Some(true)
        );
        // A second merge changes nothing.
        assert_eq!(merge_models(&mut profile, &fetched), 0);
        assert_eq!(profile.models.len(), 4);
    }

    #[test]
    fn merging_into_an_empty_provider_selects_the_first_model() {
        let mut profile = ProviderProfile::default();
        merge_models(&mut profile, &parse_models(&real_models_fixture()));
        assert_eq!(profile.model, "amazon/nova-lite-v1");
    }

    #[test]
    fn an_endpoint_must_use_the_same_scheme_as_the_base_url() {
        assert!(
            resolve_endpoint("https://localhost:8080/v1", "http://localhost:8080/models").is_err()
        );
        assert!(
            resolve_endpoint("http://localhost:8080/v1", "https://localhost:8080/models").is_err()
        );
        assert!(resolve_endpoint("http://localhost:8080/v1", "models").is_ok());
    }

    #[test]
    fn untrusted_text_loses_control_characters() {
        let models = parse_models(&json!({"data": [
            {"id": "evil\u{1b}[31m-model", "name": "line\nbreak\u{7}"}
        ]}));
        assert_eq!(models[0].id, "evil [31m-model");
        assert_eq!(models[0].name, "line break");
        let lines = summarize_limits(&json!({"note": "a\u{1b}[2Jb\r\nc"}));
        assert_eq!(lines[0].value, "a [2Jb  c");
    }

    #[test]
    fn generic_limit_values_are_capped() {
        let long = "x".repeat(500);
        let lines = summarize_limits(&json!({"note": long}));
        assert!(
            lines[0].value.chars().count() <= 80,
            "{}",
            lines[0].value.len()
        );
    }

    #[test]
    fn network_errors_never_include_the_url() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let url = format!("http://127.0.0.1:{port}/models?token=SECRET-IN-URL");
        let error = format!("{:#}", super::fetch_json(&url, "k").expect_err("refused"));
        assert!(!error.contains("SECRET-IN-URL"), "{error}");
        assert!(!error.contains("127.0.0.1"), "{error}");
    }

    #[test]
    fn oversized_responses_are_refused() {
        // Test builds use a 64 KiB limit.
        let body = "a".repeat(100 * 1024);
        let response: &'static str = Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        );
        let (url, _) = serve_once(response);
        let error = super::fetch_json(&url, "k")
            .expect_err("too large")
            .to_string();
        assert!(error.contains("too large"), "{error}");
    }

    #[test]
    fn key_shape_reports_length_and_common_paste_problems_without_revealing_the_key() {
        let prefix = Some("ma-live-");
        assert_eq!(
            super::describe_key("ma-live-abcd", prefix),
            "12 characters · looks well-formed"
        );
        assert_eq!(
            super::describe_key("abc", None),
            "3 characters · looks well-formed"
        );
        assert_eq!(super::describe_key("", prefix), "empty");
        let quoted = super::describe_key("\"ma-live-abcd\"", prefix);
        assert!(quoted.contains("wrapped in quotes"), "{quoted}");
        assert!(quoted.contains("does not start with ma-live-"), "{quoted}");
        assert!(super::describe_key("ma-live-ab cd", prefix).contains("spaces or line breaks"));
        assert!(super::describe_key("Bearer ma-live-abcd", prefix).contains("starts with Bearer"));
        assert!(super::describe_key("sk-abcd", prefix).contains("does not start with ma-live-"));
        for text in ["ma-live-abcd", "\"ma-live-abcd\"", "Bearer ma-live-abcd"] {
            assert!(!super::describe_key(text, prefix).contains("abcd"));
        }
    }

    #[test]
    fn a_rejected_key_is_explained_and_other_errors_pass_through() {
        let rejected = super::friendly_fetch_error(
            "provider returned 401 Unauthorized: The provided API key is invalid or has been revoked",
        );
        assert!(rejected.contains("rejected the API key"), "{rejected}");
        assert!(rejected.contains("press e"), "{rejected}");
        assert!(
            super::friendly_fetch_error("provider returned 403 Forbidden: no")
                .contains("rejected the API key")
        );
        assert_eq!(
            super::friendly_fetch_error("provider returned 500 Internal Server Error: boom"),
            "provider returned 500 Internal Server Error: boom"
        );
        assert_eq!(
            super::friendly_fetch_error("the provider response is too large"),
            "the provider response is too large"
        );
    }

    #[test]
    fn model_tags_show_free_no_tools_and_context() {
        use crate::ModelInfo;
        assert_eq!(super::model_tags(None), "");
        assert_eq!(super::model_tags(Some(&ModelInfo::default())), "");
        let info = ModelInfo {
            free: Some(true),
            tools: Some(false),
            context: Some(262_144),
        };
        assert_eq!(
            super::model_tags(Some(&info)),
            " · free · no tools · 262k ctx"
        );
        let paid = ModelInfo {
            free: Some(false),
            tools: Some(true),
            context: Some(1_048_576),
        };
        assert_eq!(super::model_tags(Some(&paid)), " · 1M ctx");
    }

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(3_000_000), "3,000,000");
        assert_eq!(group_digits(1_234_567_890), "1,234,567,890");
    }

    fn real_limits_fixture() -> Value {
        serde_json::from_str(
            r#"{"subscription":{"active":false,"available":false,"limits":{"five_hour":{"enabled":false,"remaining_percent":0},"weekly":{"enabled":false,"used_tokens":null,"limit_tokens":null,"remaining_tokens":null,"remaining_percent":0},"monthly":{"enabled":false,"used_tokens":null,"limit_tokens":null,"remaining_tokens":null,"remaining_percent":0}}},"billing_mode":"tokens","account_balance":{"tokens":3000000},"api_key":{"tokens_spent":0,"token_limit":null,"limit_remaining":null,"unlimited":true},"generated_at":"2026-10-07T06:14:44.630618367Z"}"#,
        )
        .expect("fixture")
    }

    fn find<'a>(lines: &'a [super::LimitLine], label: &str) -> &'a super::LimitLine {
        lines
            .iter()
            .find(|line| line.label == label)
            .unwrap_or_else(|| panic!("no {label:?} line in {lines:?}"))
    }

    #[test]
    fn real_pay_as_you_go_limits_show_balance_and_no_plan() {
        let lines = summarize_limits(&real_limits_fixture());
        assert_eq!(find(&lines, "Balance").value, "3,000,000 tokens");
        assert_eq!(find(&lines, "Plan").value, "no active subscription");
        assert_eq!(find(&lines, "This key").value, "no limit · spent 0 tokens");
        assert!(lines.iter().all(|line| line.label != "Weekly"));
    }

    #[test]
    fn enabled_windows_show_usage_and_remaining_bar() {
        let value = json!({
            "subscription": {"active": true, "limits": {
                "five_hour": {"enabled": true, "remaining_percent": 80},
                "weekly": {"enabled": true, "used_tokens": 250000, "limit_tokens": 1000000,
                           "remaining_tokens": 750000, "remaining_percent": 75,
                           "resets_at": "2026-10-14T06:00:00Z"},
                "monthly": {"enabled": false, "remaining_percent": 0}
            }},
            "account_balance": {"tokens": 12},
            "api_key": {"tokens_spent": 5, "token_limit": 100, "limit_remaining": 95, "unlimited": false}
        });
        let lines = summarize_limits(&value);
        assert_eq!(find(&lines, "Plan").value, "active");
        let weekly = find(&lines, "Weekly");
        assert!(
            weekly.value.starts_with("250,000 / 1,000,000 tokens"),
            "{}",
            weekly.value
        );
        assert!(
            weekly.value.contains("resets 2026-10-14 06:00"),
            "{}",
            weekly.value
        );
        assert_eq!(weekly.remaining, Some(0.75));
        assert_eq!(find(&lines, "5-hour").remaining, Some(0.8));
        assert!(lines.iter().all(|line| line.label != "Monthly"));
        assert_eq!(find(&lines, "This key").value, "5 / 100 tokens · 95 left");
    }

    #[test]
    fn unknown_shapes_fall_back_to_listing_scalar_fields() {
        let lines = summarize_limits(&json!({"quota": {"used": 5, "max": 10, "note": "ok"}}));
        assert_eq!(find(&lines, "quota.used").value, "5");
        assert_eq!(find(&lines, "quota.note").value, "ok");
        assert!(summarize_limits(&json!(null)).is_empty());
    }

    /// Serves one canned HTTP response on a loopback port and returns (url, received request).
    fn serve_once(response: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/models", listener.local_addr().expect("addr"));
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).expect("read");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
            stream.write_all(response.as_bytes()).expect("write");
        });
        (url, receiver)
    }

    #[test]
    fn fetching_sends_the_key_as_a_bearer_token_and_parses_json() {
        let (url, request) = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 14\r\nConnection: close\r\n\r\n{\"data\":[1,2]}",
        );
        let value = super::fetch_json(&url, "test-key-123").expect("fetch");
        assert_eq!(value["data"][1], 2);
        let request = request.recv().expect("request").to_lowercase();
        assert!(
            request.contains("authorization: bearer test-key-123"),
            "{request}"
        );
        assert!(request.starts_with("get /models"), "{request}");
    }

    #[test]
    fn provider_errors_are_reported_without_echoing_the_key() {
        let (url, _) = serve_once(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 28\r\nConnection: close\r\n\r\nbad key test-key-123 given!!",
        );
        let error = super::fetch_json(&url, "test-key-123")
            .expect_err("401")
            .to_string();
        assert!(error.contains("401"), "{error}");
        assert!(!error.contains("test-key-123"), "{error}");
    }

    #[test]
    fn redirects_are_refused_so_the_key_cannot_follow_them() {
        let (url, _) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: https://evil.example/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let error = super::fetch_json(&url, "test-key-123")
            .expect_err("redirect")
            .to_string();
        assert!(error.contains("redirected"), "{error}");
    }

    #[test]
    fn fetched_model_struct_is_comparable() {
        let model = FetchedModel {
            id: "a".to_owned(),
            name: String::new(),
            free: None,
            tools: None,
            context: None,
        };
        assert_eq!(model.clone(), model);
    }
}
