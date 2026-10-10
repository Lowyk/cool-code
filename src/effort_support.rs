//! Which effort levels a model really has, and how a chosen level becomes a request parameter.
//!
//! Providers disagree on names (`reasoning_effort`, `thinkingLevel`, a token budget) and not every
//! model has an effort control at all, so showing every level for every model would promise
//! things that do nothing. The table below says what is known; anything unknown has no levels, and
//! a model that rejects the parameter anyway is remembered so the request is not retried with it.

use crate::Effort;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Mutex;

/// The five levels a model itself can offer, lowest first. `Super` and `Ultimate` are not model
/// levels: they are `XHigh` and `Max` with the harness's workflows switched on.
pub(crate) const MODEL_LEVELS: [Effort; 5] = [
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

impl Effort {
    /// The model-level effort this setting asks for (`Super` is `XHigh` plus workflows).
    pub(crate) fn model_level(self) -> Effort {
        match self {
            Effort::Super => Effort::XHigh,
            Effort::Ultimate => Effort::Max,
            other => other,
        }
    }

    /// Whether this setting is one of the two workflow tiers.
    pub(crate) fn is_workflow_tier(self) -> bool {
        matches!(self, Effort::Super | Effort::Ultimate)
    }
}

impl crate::Settings {
    /// Puts the effort back inside what is unlocked: while the workflow size is Off, Super and
    /// Ultimate become XHigh and Max and workflows are switched off. Returns whether anything
    /// had to change.
    pub(crate) fn enforce_workflow_lock(&mut self) -> bool {
        if self.workflows_unlocked() {
            return false;
        }
        let before = (self.effort, self.workflows);
        self.effort = self.effort.model_level();
        self.workflows = false;
        before != (self.effort, self.workflows)
    }

    /// Whether workflows (subagents) are on for the current effort.
    pub(crate) fn workflows_active(&self) -> bool {
        self.workflows_unlocked()
            && (self.effort.is_workflow_tier()
                || (self.workflows
                    && matches!(self.effort, Effort::Low | Effort::Medium | Effort::High)))
    }
}

/// Where in the model list's ordering a level sits.
fn rank(level: Effort) -> usize {
    MODEL_LEVELS
        .iter()
        .position(|candidate| *candidate == level.model_level())
        .unwrap_or(0)
}

/// The numeric version in a model name (`claude-opus-5-5` is `[5, 5]`, `gemini-2.5-pro` is
/// `[2, 5]`); empty when there is none.
fn version_of(name: &str) -> Vec<u32> {
    let mut version = Vec::new();
    for token in name.split(['-', '_']) {
        match token
            .split('.')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(numbers) => version.extend(numbers),
            Err(_) if version.is_empty() => {}
            Err(_) => break,
        }
    }
    version
}

/// The model-id part after any `author/` prefix, lower-cased.
fn bare(model_id: &str) -> String {
    model_id
        .rsplit('/')
        .next()
        .unwrap_or(model_id)
        .to_ascii_lowercase()
}

/// The effort levels `model_id` offers. Empty means the model has no adjustable effort (or it is
/// not known to have any).
pub(crate) fn supported_levels(model_id: &str, overrides: &[Effort]) -> Vec<Effort> {
    if !overrides.is_empty() {
        let mut chosen = overrides
            .iter()
            .map(|level| level.model_level())
            .collect::<Vec<_>>();
        chosen.sort_by_key(|level| rank(*level));
        chosen.dedup();
        return chosen;
    }
    let name = bare(model_id);
    let version = version_of(&name);
    let major = version.first().copied().unwrap_or(0);
    use Effort::{High, Low, Max, Medium, XHigh};
    if name.starts_with("claude") {
        if name.contains("haiku") {
            return Vec::new();
        }
        return if major >= 5 {
            vec![Low, Medium, High, XHigh, Max]
        } else {
            vec![Low, Medium, High]
        };
    }
    if name.starts_with("gpt-oss") {
        return vec![Low, Medium, High];
    }
    if name.starts_with("gpt-5") || name.starts_with("gpt-6") || name.starts_with("gpt-7") {
        // xhigh arrived with GPT-5.4 (and the Codex max models), max with GPT-5.6 and GPT-6.
        let minor = version.get(1).copied().unwrap_or(0);
        let from = |major_needed: u32, minor_needed: u32| {
            major > major_needed || (major == major_needed && minor >= minor_needed)
        };
        let mut levels = vec![Low, Medium, High];
        if from(5, 4) || name.contains("codex-max") {
            levels.push(XHigh);
        }
        if from(5, 6) {
            levels.push(Max);
        }
        return levels;
    }
    if name.starts_with("o1") || name.starts_with("o3") || name.starts_with("o4") {
        return vec![Low, Medium, High];
    }
    if name.starts_with("gemini") {
        if major >= 3 {
            return if name.contains("pro") {
                vec![Low, High]
            } else {
                vec![Low, Medium, High]
            };
        }
        if name.starts_with("gemini-2.5") || name.starts_with("gemini-2-5") {
            return if name.contains("pro") {
                vec![Low, Medium, High, XHigh, Max]
            } else {
                vec![Low, Medium, High]
            };
        }
        return Vec::new();
    }
    if name.starts_with("grok-3-mini") {
        return vec![Low, High];
    }
    if takes_low_high_max(&name) {
        return vec![Low, High, Max];
    }
    Vec::new()
}

/// DeepSeek V4 and V4.1 and Kimi K3 take `reasoning_effort` as "low", "high" or "max", with
/// "max" meaning their own highest effort (not OpenAI's "xhigh"). Kimi K2.x and the retired
/// `deepseek-chat` / `deepseek-reasoner` have no effort control (K2.x only turns thinking on or
/// off), so they are not listed.
///
/// - DeepSeek: `deepseek-flash` and `deepseek-v4-pro`, plus the legacy `deepseek-v4-flash` names
///   that route to V4.1 Flash (api-docs.deepseek.com, Chat Completions and Lists Models).
/// - Kimi API: `kimi-k3` (platform.kimi.ai, Reasoning Effort and Model Parameter Reference).
/// - Kimi Code membership: `k3`, `k3-256k` and `kimi-for-coding` list low / high / max effort;
///   `kimi-for-coding-highspeed` (K2.7 Code) does not (kimi.com Kimi Code membership guide).
fn takes_low_high_max(name: &str) -> bool {
    let deepseek = ["deepseek-flash", "deepseek-v4-pro", "deepseek-v4-flash"]
        .iter()
        .any(|family| name == *family || name.starts_with(&format!("{family}-")));
    let kimi = name == "kimi-k3"
        || name.starts_with("kimi-k3-")
        || name == "k3"
        || name.starts_with("k3-")
        || name == "kimi-for-coding";
    deepseek || kimi
}

/// The level the model will actually run at when `wanted` is chosen: the highest level the model
/// has that does not exceed it (or its lowest, when everything it has is higher). `None` when the
/// model has no levels.
pub(crate) fn nearest_level(wanted: Effort, supported: &[Effort]) -> Option<Effort> {
    let wanted_rank = rank(wanted);
    supported
        .iter()
        .copied()
        .filter(|level| rank(*level) <= wanted_rank)
        .max_by_key(|level| rank(*level))
        .or_else(|| supported.iter().copied().min_by_key(|level| rank(*level)))
}

/// Which API a request goes to, for choosing the parameter's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Api {
    OpenAiCompatible,
    OpenRouter,
    Anthropic,
    Google,
    /// The ChatGPT sign-in backend, which speaks the Responses format.
    ChatGpt,
}

fn name_of(level: Effort) -> &'static str {
    match level.model_level() {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::XHigh => "xhigh",
        _ => "max",
    }
}

/// Puts the chosen effort into `body`. Returns whether anything was added, so the caller knows a
/// retry without it is possible.
pub(crate) fn apply_effort(
    body: &mut Value,
    api: Api,
    model_id: &str,
    wanted: Effort,
    overrides: &[Effort],
) -> bool {
    let supported = supported_levels(model_id, overrides);
    let Some(level) = nearest_level(wanted, &supported) else {
        return false;
    };
    if rejected(api, model_id) {
        return false;
    }
    match api {
        Api::OpenAiCompatible => {
            // OpenAI's own scale tops out at "xhigh"; DeepSeek and Kimi take "max" itself.
            let text = if level.model_level() == Effort::Max && !takes_low_high_max(&bare(model_id))
            {
                "xhigh"
            } else {
                name_of(level)
            };
            body["reasoning_effort"] = json!(text);
        }
        Api::ChatGpt => {
            // The Responses format takes "max" itself; chat completions stops at "xhigh".
            body["reasoning"] = json!({ "effort": name_of(level), "summary": "auto" });
        }
        Api::OpenRouter => {
            let text = if level.model_level() == Effort::Max {
                "xhigh"
            } else {
                name_of(level)
            };
            body["reasoning"] = json!({ "effort": text });
        }
        Api::Anthropic => {
            body["output_config"] = json!({ "effort": name_of(level) });
        }
        Api::Google => {
            let budget = [1_024, 4_096, 8_192, 16_384, 32_768][rank(level)];
            let thinking = if version_of(&bare(model_id)).first().copied().unwrap_or(0) >= 3 {
                json!({ "thinkingLevel": if rank(level) >= rank(Effort::High) { "high" } else if rank(level) == 0 { "low" } else { "medium" } })
            } else {
                json!({ "thinkingBudget": budget })
            };
            body["generationConfig"] = json!({ "thinkingConfig": thinking });
        }
    }
    true
}

static REJECTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn key(api: Api, model_id: &str) -> String {
    format!("{api:?}/{}", model_id.to_ascii_lowercase())
}

/// Remembers that this model did not accept an effort parameter, for the rest of the run.
pub(crate) fn remember_rejected(api: Api, model_id: &str) {
    let mut guard = REJECTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .get_or_insert_with(HashSet::new)
        .insert(key(api, model_id));
}

pub(crate) fn rejected(api: Api, model_id: &str) -> bool {
    let guard = REJECTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .as_ref()
        .is_some_and(|set| set.contains(&key(api, model_id)))
}

/// Whether an error body from a provider is complaining about the effort parameter we sent.
pub(crate) fn complains_about_effort(error_text: &str) -> bool {
    let lower = error_text.to_ascii_lowercase();
    [
        "effort",
        "reasoning",
        "thinking",
        "output_config",
        "thinkingconfig",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Effort::{High, Low, Max, Medium, Super, Ultimate, XHigh};

    #[test]
    fn workflow_tiers_are_xhigh_and_max_with_workflows_on() {
        assert_eq!(Super.model_level(), XHigh);
        assert_eq!(Ultimate.model_level(), Max);
        assert_eq!(High.model_level(), High);
        assert!(Super.is_workflow_tier() && Ultimate.is_workflow_tier() && !Max.is_workflow_tier());
    }

    #[test]
    fn workflow_tiers_are_locked_until_dynamic_workflows_are_enabled() {
        let mut settings = crate::Settings::default();
        assert!(!settings.workflows_unlocked(), "locked by default");
        settings.effort = Ultimate;
        settings.workflows = true;
        assert!(settings.enforce_workflow_lock());
        assert_eq!((settings.effort, settings.workflows), (Max, false));
        settings.effort = Super;
        assert!(settings.enforce_workflow_lock());
        assert_eq!(settings.effort, XHigh);
        settings.effort = High;
        assert!(
            !settings.enforce_workflow_lock(),
            "ordinary levels are untouched"
        );
        assert!(!settings.workflows_active());
    }

    #[test]
    fn unlocked_workflows_are_active_on_the_tiers_and_on_ticked_lower_levels() {
        let mut settings = crate::Settings::default();
        settings.workflow_size = crate::workflow::WorkflowSize::Small;
        for (effort, ticked, expected) in [
            (Super, false, true),
            (Ultimate, false, true),
            (High, true, true),
            (Low, true, true),
            (High, false, false),
            (XHigh, true, false),
            (Max, true, false),
        ] {
            settings.effort = effort;
            settings.workflows = ticked;
            assert_eq!(
                settings.workflows_active(),
                expected,
                "{effort:?} ticked={ticked}"
            );
        }
        settings.workflow_size = crate::workflow::WorkflowSize::Off;
        settings.effort = Super;
        assert!(
            !settings.workflows_active(),
            "locking switches everything off"
        );
        assert!(!settings.enforce_workflow_lock() || settings.effort == XHigh);
    }

    #[test]
    fn models_without_effort_controls_offer_no_levels() {
        for id in [
            "deepseek-chat",
            "deepseek-reasoner",
            "deepseek/deepseek-reasoner",
            "deepseek-v4",
            "kimi-k2.6",
            "kimi-k2.5",
            "kimi-k2.7-code",
            "kimi-k2.7-code-highspeed",
            "kimi-for-coding-highspeed",
            "kimi-k2-thinking",
            "qwen3.8-max",
            "glm-5.3-flash",
            "claude-haiku-4-5",
            "gpt-4o",
            "gemini-2.0-flash",
            "mystery-model",
        ] {
            assert!(supported_levels(id, &[]).is_empty(), "{id}");
        }
    }

    #[test]
    fn known_reasoning_models_offer_their_own_levels() {
        assert_eq!(
            supported_levels("claude-opus-5-5", &[]),
            [Low, Medium, High, XHigh, Max]
        );
        assert_eq!(
            supported_levels("anthropic/claude-sonnet-4-5", &[]),
            [Low, Medium, High]
        );
        // xhigh came with GPT-5.4 and max with GPT-5.6 / GPT-6.
        assert_eq!(supported_levels("gpt-5", &[]), [Low, Medium, High]);
        assert_eq!(
            supported_levels("gpt-5.1-codex-max", &[]),
            [Low, Medium, High, XHigh]
        );
        assert_eq!(supported_levels("gpt-5.5", &[]), [Low, Medium, High, XHigh]);
        for model in ["gpt-5.6-sol", "gpt-6-astra", "gpt-6-luna", "gpt-6.1-sol"] {
            assert_eq!(
                supported_levels(model, &[]),
                [Low, Medium, High, XHigh, Max],
                "{model}"
            );
        }
        assert_eq!(
            supported_levels("openai/gpt-oss-120b", &[]),
            [Low, Medium, High]
        );
        assert_eq!(supported_levels("o3-mini", &[]), [Low, Medium, High]);
        assert_eq!(supported_levels("gemini-3-pro", &[]), [Low, High]);
        assert_eq!(supported_levels("gemini-3-flash", &[]), [Low, Medium, High]);
        assert_eq!(
            supported_levels("gemini-2.5-pro", &[]),
            [Low, Medium, High, XHigh, Max]
        );
    }

    #[test]
    fn documented_deepseek_and_kimi_models_offer_low_high_and_max() {
        for id in [
            "deepseek-flash",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "deepseek-v4-flash-vision-exp",
            "deepseek/deepseek-v4-pro",
            "kimi-k3",
            "moonshotai/kimi-k3",
            "k3",
            "k3-256k",
            "kimi-for-coding",
        ] {
            assert_eq!(supported_levels(id, &[]), [Low, High, Max], "{id}");
        }
    }

    #[test]
    fn a_manual_override_wins_and_is_ordered() {
        assert_eq!(supported_levels("deepseek-v4", &[High, Low]), [Low, High]);
        assert_eq!(supported_levels("claude-opus-5", &[Ultimate]), [Max]);
    }

    #[test]
    fn a_choice_runs_at_the_nearest_level_the_model_has() {
        let gemini = [Low, High];
        assert_eq!(nearest_level(Low, &gemini), Some(Low));
        assert_eq!(nearest_level(Medium, &gemini), Some(Low), "rounds down");
        assert_eq!(
            nearest_level(Max, &gemini),
            Some(High),
            "capped at the model's best"
        );
        assert_eq!(nearest_level(Ultimate, &gemini), Some(High));
        assert_eq!(
            nearest_level(Low, &[High, XHigh]),
            Some(High),
            "below everything: lowest"
        );
        assert_eq!(nearest_level(High, &[]), None);
    }

    #[test]
    fn each_api_gets_its_own_parameter_shape() {
        let apply = |api, model: &str, level| {
            let mut body = json!({"model": model});
            let added = apply_effort(&mut body, api, model, level, &[]);
            (added, body)
        };
        let (added, body) = apply(Api::OpenAiCompatible, "gpt-6-astra", High);
        assert!(added);
        assert_eq!(body["reasoning_effort"], "high");
        let (_, body) = apply(Api::OpenAiCompatible, "gpt-6-astra", Ultimate);
        assert_eq!(
            body["reasoning_effort"], "xhigh",
            "capped at the model's best"
        );
        let (_, body) = apply(Api::ChatGpt, "gpt-6-astra", Max);
        assert_eq!(body["reasoning"]["effort"], "max");
        assert_eq!(body["reasoning"]["summary"], "auto");
        let (_, body) = apply(Api::OpenRouter, "openai/gpt-6-astra", Medium);
        assert_eq!(body["reasoning"]["effort"], "medium");
        let (_, body) = apply(Api::Anthropic, "claude-opus-5-5", Ultimate);
        assert_eq!(body["output_config"]["effort"], "max");
        let (_, body) = apply(Api::Google, "gemini-3-pro", Max);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "high"
        );
        let (_, body) = apply(Api::Google, "gemini-2.5-pro", Low);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            1024
        );
    }

    #[test]
    fn deepseek_and_kimi_are_sent_their_own_level_names_including_max() {
        let sent = |api, model: &str, level| {
            let mut body = json!({"model": model});
            assert!(apply_effort(&mut body, api, model, level, &[]), "{model}");
            body
        };
        for model in [
            "deepseek-flash",
            "deepseek-v4-pro",
            "kimi-k3",
            "kimi-for-coding",
            "k3",
        ] {
            let compatible = Api::OpenAiCompatible;
            assert_eq!(sent(compatible, model, Low)["reasoning_effort"], "low");
            assert_eq!(sent(compatible, model, High)["reasoning_effort"], "high");
            assert_eq!(
                sent(compatible, model, Max)["reasoning_effort"],
                "max",
                "{model} takes max itself, not OpenAI's xhigh"
            );
            assert_eq!(sent(compatible, model, Ultimate)["reasoning_effort"], "max");
            assert_eq!(
                sent(compatible, model, Medium)["reasoning_effort"],
                "low",
                "{model} has no medium, so it rounds down"
            );
        }
        let anthropic = sent(Api::Anthropic, "deepseek-flash", Max);
        assert_eq!(anthropic["output_config"]["effort"], "max");
        assert_eq!(
            sent(Api::OpenAiCompatible, "gpt-6-astra", Max)["reasoning_effort"],
            "xhigh",
            "OpenAI's own models still stop at xhigh"
        );
    }

    #[test]
    fn a_model_without_levels_gets_no_parameter_at_all() {
        let mut body = json!({"model": "deepseek-v4"});
        assert!(!apply_effort(
            &mut body,
            Api::OpenAiCompatible,
            "deepseek-v4",
            Max,
            &[]
        ));
        assert_eq!(body, json!({"model": "deepseek-v4"}));
    }

    #[test]
    fn a_rejected_model_stops_getting_the_parameter() {
        let model = "gpt-6-rejects-effort-test";
        let mut body = json!({});
        assert!(apply_effort(
            &mut body,
            Api::OpenAiCompatible,
            model,
            High,
            &[]
        ));
        remember_rejected(Api::OpenAiCompatible, model);
        let mut again = json!({});
        assert!(!apply_effort(
            &mut again,
            Api::OpenAiCompatible,
            model,
            High,
            &[]
        ));
        assert_eq!(again, json!({}));
        assert!(
            apply_effort(&mut json!({}), Api::OpenRouter, model, High, &[]),
            "another API is unaffected"
        );
    }

    #[test]
    fn provider_complaints_about_the_parameter_are_recognized() {
        assert!(complains_about_effort(
            "Unknown parameter: 'reasoning_effort'."
        ));
        assert!(complains_about_effort(
            "{\"error\":\"thinking is not supported\"}"
        ));
        assert!(!complains_about_effort("Invalid API key"));
        assert!(!complains_about_effort("model not found"));
    }
}
