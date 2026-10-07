//! Resolves short model-series references such as `fable` or `fable-5` to the newest matching
//! model that the configured providers list.

use crate::Settings;

/// Returns the provider/model pairs that best answer `query`.
///
/// A query is one or more name words plus an optional version: `fable` selects the newest model
/// whose name contains that word, `fable-5` only version 5 (which equals 5.0), and `fable-5.1` or
/// `fable-5-1` only version 5.1. Several entries come back only when the same model is offered by
/// more than one provider; an empty result means nothing matched.
pub(super) fn resolve_series(settings: &Settings, query: &str) -> Vec<(usize, String)> {
    let wanted = ParsedName::new(query);
    if wanted.words.is_empty() {
        return Vec::new();
    }
    // (version, extra words, provider index, id) for every model that fits the query.
    let mut candidates = Vec::new();
    for (index, profile) in settings
        .providers
        .iter()
        .enumerate()
        .filter(|(_, profile)| !profile.draft)
    {
        for model in &profile.models {
            let found = ParsedName::new(model.id.rsplit('/').next().unwrap_or(&model.id));
            let has_words = wanted.words.iter().all(|word| found.words.contains(word));
            let has_version = wanted.version.is_empty() || wanted.version == found.version;
            if has_words && has_version {
                let extra = found.words.len() - wanted.words.len();
                candidates.push((found.version, extra, index, model.id.clone()));
            }
        }
    }
    let Some(best) = candidates
        .iter()
        .min_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)))
        .cloned()
    else {
        return Vec::new();
    };
    let tied = candidates
        .into_iter()
        .filter(|(version, extra, _, _)| *version == best.0 && *extra == best.1)
        .collect::<Vec<_>>();
    // Different models that fit equally well (the same name from two authors) are not guessed.
    if tied
        .iter()
        .any(|(_, _, _, id)| !id.eq_ignore_ascii_case(&best.3))
    {
        return Vec::new();
    }
    tied.into_iter()
        .map(|(_, _, index, id)| (index, id))
        .collect()
}

/// The numeric version in a model id (`claude-opus-5-5` is `[5, 5]`); empty when it has none.
pub(super) fn version_of(model_id: &str) -> Vec<u32> {
    ParsedName::new(model_id.rsplit('/').next().unwrap_or(model_id)).version
}

/// A model name split into lowercase words and a numeric version (`5.1` is `[5, 1]`).
struct ParsedName {
    words: Vec<String>,
    version: Vec<u32>,
}

impl ParsedName {
    fn new(name: &str) -> ParsedName {
        let mut words = Vec::new();
        let mut version = Vec::new();
        // True while the version is still being read, so `5-1` becomes 5.1 but `3-70b` does not.
        let mut reading_version = false;
        for token in name
            .to_ascii_lowercase()
            .split(['-', '_', ' '])
            .filter(|token| !token.is_empty())
        {
            if let Some(numbers) = numbers(token) {
                if version.is_empty() || reading_version {
                    version.extend(numbers);
                    reading_version = true;
                }
                continue;
            }
            reading_version = false;
            // `k2.7` and `qwen3.8` carry the version after a few letters.
            let split = token.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
            match numbers(&token[split..]) {
                Some(numbers) if split > 0 => {
                    words.push(token[..split].to_owned());
                    if version.is_empty() {
                        version = numbers;
                    }
                }
                _ => words.push(token.to_owned()),
            }
        }
        while version.last() == Some(&0) {
            version.pop();
        }
        ParsedName { words, version }
    }
}

/// Parses `5` or `5.1` into its parts; anything else (`70b`, `k2`) is not a plain number.
fn numbers(token: &str) -> Option<Vec<u32>> {
    token.split('.').map(|part| part.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::resolve_series;
    use crate::{ModelProfile, ProviderProfile, Settings};

    fn provider(id: &str, models: &[&str]) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: id.to_owned(),
            adapter: "openai".to_owned(),
            model: models.first().map(|m| (*m).to_owned()).unwrap_or_default(),
            models: models
                .iter()
                .map(|id| ModelProfile {
                    id: (*id).to_owned(),
                    name: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn settings(providers: Vec<ProviderProfile>) -> Settings {
        Settings {
            providers,
            ..Default::default()
        }
    }

    fn ids(found: Vec<(usize, String)>) -> Vec<String> {
        found.into_iter().map(|(_, id)| id).collect()
    }

    fn multiai() -> Settings {
        settings(vec![provider(
            "multiai",
            &[
                "claude-fable-5",
                "claude-fable-5-1",
                "claude-fable-4-6",
                "claude-opus-5-5",
                "claude-opus-5-5-thinking",
                "gemini-3-flash",
                "gemini-3-flash-lite",
                "gemini-3-pro",
                "kimi-k2.7-code",
                "kimi-k3",
                "gpt-6-astra",
                "gpt-oss-120b",
            ],
        )])
    }

    #[test]
    fn a_bare_series_name_picks_the_newest_version() {
        assert_eq!(
            ids(resolve_series(&multiai(), "fable")),
            ["claude-fable-5-1"]
        );
        assert_eq!(
            ids(resolve_series(&multiai(), "Fable")),
            ["claude-fable-5-1"]
        );
        assert_eq!(
            ids(resolve_series(&multiai(), "claude-fable")),
            ["claude-fable-5-1"]
        );
        assert_eq!(ids(resolve_series(&multiai(), "kimi")), ["kimi-k3"]);
    }

    #[test]
    fn a_whole_number_means_exactly_that_version() {
        assert_eq!(
            ids(resolve_series(&multiai(), "fable-5")),
            ["claude-fable-5"]
        );
        assert_eq!(
            ids(resolve_series(&multiai(), "fable-5.1")),
            ["claude-fable-5-1"]
        );
        assert_eq!(
            ids(resolve_series(&multiai(), "fable-5-1")),
            ["claude-fable-5-1"]
        );
        assert_eq!(
            ids(resolve_series(&multiai(), "fable 4.6")),
            ["claude-fable-4-6"]
        );
        assert_eq!(
            ids(resolve_series(&multiai(), "kimi-k2.7")),
            ["kimi-k2.7-code"]
        );
        assert!(resolve_series(&multiai(), "fable-9").is_empty());
    }

    #[test]
    fn plain_models_beat_their_variants() {
        assert_eq!(ids(resolve_series(&multiai(), "opus")), ["claude-opus-5-5"]);
        assert_eq!(ids(resolve_series(&multiai(), "flash")), ["gemini-3-flash"]);
        assert_eq!(
            ids(resolve_series(&multiai(), "flash-lite")),
            ["gemini-3-flash-lite"]
        );
    }

    #[test]
    fn unrelated_text_matches_nothing() {
        assert!(resolve_series(&multiai(), "banana").is_empty());
        assert!(resolve_series(&multiai(), "").is_empty());
        assert!(resolve_series(&multiai(), "5").is_empty());
    }

    #[test]
    fn author_prefixes_and_sizes_do_not_confuse_matching() {
        let settings = settings(vec![provider(
            "groq",
            &["openai/gpt-oss-120b", "meta-llama/llama-3-70b-instruct"],
        )]);
        assert_eq!(
            ids(resolve_series(&settings, "oss")),
            ["openai/gpt-oss-120b"]
        );
        assert_eq!(
            ids(resolve_series(&settings, "llama-3")),
            ["meta-llama/llama-3-70b-instruct"]
        );
    }

    #[test]
    fn the_same_model_in_two_providers_is_returned_for_both() {
        let settings = settings(vec![
            provider("a", &["claude-fable-5-1"]),
            provider("b", &["claude-fable-5-1", "claude-fable-4"]),
        ]);
        let found = resolve_series(&settings, "fable");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, 0);
        assert_eq!(found[1].0, 1);
    }

    #[test]
    fn drafts_are_ignored_and_the_newest_wins_across_providers() {
        let mut draft = provider("draft", &["claude-fable-9"]);
        draft.draft = true;
        let settings = settings(vec![
            draft,
            provider("old", &["claude-fable-4"]),
            provider("new", &["claude-fable-5"]),
        ]);
        assert_eq!(
            resolve_series(&settings, "fable"),
            [(2, "claude-fable-5".to_owned())]
        );
    }
}
