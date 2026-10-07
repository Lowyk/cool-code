use crate::tui::state::App;
use crate::{ChainModel, ProviderProfile, Settings, write_settings};
use anyhow::Result;

pub(super) fn find_model_matches(settings: &Settings, model_id: &str) -> Vec<(usize, String)> {
    let mut matches = Vec::new();
    for (index, profile) in settings.providers.iter().enumerate() {
        if profile.draft {
            continue;
        }
        let is_in_chain = settings.model_chains.iter().any(|chain| {
            chain.members.iter().any(|member| {
                member.provider_id == profile.id && member.model_id.eq_ignore_ascii_case(model_id)
            })
        });
        if !profile.auto_switch && !is_in_chain {
            continue;
        }
        if let Some(id) = model_id_for_profile(profile, model_id) {
            matches.push((index, id));
        }
    }
    matches
}

pub(super) fn find_model_matches_all(settings: &Settings, model_id: &str) -> Vec<(usize, String)> {
    settings
        .providers
        .iter()
        .enumerate()
        .filter_map(|(index, profile)| {
            (!profile.draft)
                .then(|| model_id_for_profile(profile, model_id))
                .flatten()
                .map(|id| (index, id))
        })
        .collect()
}

pub(super) fn available_chain_models(settings: &Settings) -> Vec<(ChainModel, String)> {
    let mut models = Vec::new();
    for profile in settings.providers.iter().filter(|profile| !profile.draft) {
        if profile.models.is_empty() && !profile.model.is_empty() {
            models.push((
                ChainModel {
                    provider_id: profile.id.clone(),
                    model_id: profile.model.clone(),
                },
                format!("{} · {}", profile.name, model_name(&profile.model)),
            ));
        } else {
            for model in &profile.models {
                models.push((
                    ChainModel {
                        provider_id: profile.id.clone(),
                        model_id: model.id.clone(),
                    },
                    format!(
                        "{} · {}",
                        profile.name,
                        if model.name.trim().is_empty() {
                            model_name(&model.id)
                        } else {
                            model.name.clone()
                        }
                    ),
                ));
            }
        }
    }
    models
}

pub(super) fn slug(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub(super) fn model_id_for_profile(profile: &ProviderProfile, model_id: &str) -> Option<String> {
    if profile.models.is_empty() && !profile.model.is_empty() {
        return profile
            .model
            .eq_ignore_ascii_case(model_id)
            .then(|| profile.model.clone());
    }
    if let Some(exact) = profile
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(model_id))
    {
        return Some(exact.id.clone());
    }
    // Providers such as Groq register author-prefixed IDs (`openai/gpt-oss-120b`);
    // accept the bare name only when it identifies exactly one model.
    let mut suffix_matches = profile.models.iter().filter(|model| {
        model
            .id
            .rsplit_once('/')
            .is_some_and(|(_, name)| name.eq_ignore_ascii_case(model_id))
    });
    match (suffix_matches.next(), suffix_matches.next()) {
        (Some(model), None) => Some(model.id.clone()),
        _ => None,
    }
}

pub(super) fn resolve_model_reference(
    settings: &Settings,
    requested: &str,
) -> (String, Vec<(usize, String)>) {
    let mut resolved_id = requested.to_owned();
    let mut matches = find_model_matches(settings, requested);
    if let Some((author, unprefixed)) = requested.split_once('/')
        && model_author_matches(author, unprefixed)
    {
        resolved_id = unprefixed.to_owned();
        for matched in find_model_matches(settings, unprefixed) {
            if !matches
                .iter()
                .any(|(provider, id)| *provider == matched.0 && id == &matched.1)
            {
                matches.push(matched);
            }
        }
    }
    if matches.is_empty() {
        let fallback = settings
            .default_provider_id
            .as_deref()
            .or(settings.active_provider_id.as_deref())
            .and_then(|id| {
                settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == id && !profile.draft)
            });
        if let Some(profile) = fallback
            && let Some(id) = model_id_for_profile(profile, &resolved_id)
            && let Some(index) = settings
                .providers
                .iter()
                .position(|candidate| candidate.id == profile.id)
        {
            matches.push((index, id));
        }
    }
    // A series name (`fable`, `fable-5`) picks the newest listed model of that series.
    if matches.is_empty() {
        matches = super::series::resolve_series(settings, requested);
        if let Some((_, id)) = matches.first() {
            resolved_id = id.clone();
        }
    }
    (resolved_id, matches)
}

pub(super) fn model_author_matches(author: &str, model_id: &str) -> bool {
    let author = author.to_ascii_lowercase();
    let model = model_id.to_ascii_lowercase();
    match author.as_str() {
        "anthropic" => model.starts_with("claude"),
        "openai" => ["gpt", "o1", "o3", "o4"]
            .iter()
            .any(|prefix| model.starts_with(prefix)),
        "google" | "gemini" => model.starts_with("gemini"),
        "meta" | "meta-llama" | "llama" => model.starts_with("llama"),
        "qwen" => model.starts_with("qwen"),
        "zai" | "z.ai" | "glm" => model.starts_with("glm"),
        "deepseek" => model.starts_with("deepseek"),
        _ => model.starts_with(&author),
    }
}

pub(super) fn selected_model_name(settings: &Settings, model_id: &str) -> String {
    if let Some(profile) = settings.active_provider_id.as_deref().and_then(|active| {
        settings
            .providers
            .iter()
            .find(|profile| profile.id == active)
    }) && let Some(model) = profile
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(model_id))
        && !model.name.trim().is_empty()
    {
        return model.name.clone();
    }
    model_name(model_id)
}

pub(super) fn model_display_for_profile(
    settings: &Settings,
    provider_id: &str,
    model_id: &str,
) -> String {
    if let Some(model) = settings
        .providers
        .iter()
        .find(|profile| profile.id == provider_id)
        .and_then(|profile| {
            profile
                .models
                .iter()
                .find(|model| model.id.eq_ignore_ascii_case(model_id))
        })
        && !model.name.trim().is_empty()
    {
        return model.name.clone();
    }
    model_name(model_id)
}

pub(super) fn model_name(model_id: &str) -> String {
    if let Some((author, unprefixed)) = model_id.split_once('/')
        && model_author_matches(author, unprefixed)
    {
        return model_name(unprefixed);
    }
    let series = [
        ("deepseek", "DeepSeek"),
        ("claude", "Claude"),
        ("gemini", "Gemini"),
        ("qwen", "Qwen"),
        ("glm", "GLM"),
        ("gpt", "GPT"),
        ("oss", "OSS"),
        ("llama", "Llama"),
        ("mistral", "Mistral"),
        ("codestral", "Codestral"),
        ("grok", "Grok"),
        ("kimi", "Kimi"),
        ("minimax", "MiniMax"),
        ("command", "Command"),
    ];
    let parts = model_id
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let mut output = Vec::<String>::new();
    let mut index = 0;
    while index < parts.len() {
        let part = parts[index];
        if part.chars().all(|character| character.is_ascii_digit())
            && index + 1 < parts.len()
            && parts[index + 1]
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            output.push(format!("{}.{}", part, parts[index + 1]));
            index += 2;
            continue;
        }
        let lower = part.to_ascii_lowercase();
        let stylized = series
            .iter()
            .find_map(|(key, value)| lower.strip_prefix(key).map(|suffix| (*value, suffix)));
        if let Some((name, suffix)) = stylized {
            let suffix = suffix.to_owned();
            output.push(if suffix.is_empty() {
                name.to_owned()
            } else {
                format!("{name}{suffix}")
            });
        } else if is_parameter_size(part) {
            output.push(part.to_ascii_uppercase());
        } else {
            let mut chars = part.chars();
            output.push(
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default(),
            );
        }
        index += 1;
    }
    let mut name = String::new();
    for (position, token) in output.iter().enumerate() {
        if position > 0 {
            let hyphenated = joins_with_hyphen(&output[0], &output[position - 1], token);
            name.push(if hyphenated { '-' } else { ' ' });
        }
        name.push_str(token);
    }
    name
}

/// Whether `token` is a parameter count such as `120b`, `70B` or `1.5b`.
fn is_parameter_size(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    let Some(number) = lower.strip_suffix(['b', 'k', 'm']) else {
        return false;
    };
    let mut halves = number.split('.');
    let digits = |half: Option<&str>| {
        half.is_some_and(|text| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()))
    };
    digits(halves.next())
        && (halves.clone().next().is_none() || digits(halves.next()))
        && halves.next().is_none()
}

/// Series whose display names keep a hyphen between words (`GPT-6 Astra`, `Flash-Lite`).
fn joins_with_hyphen(first: &str, previous: &str, next: &str) -> bool {
    first.starts_with("Qwen")
        || (previous == "GPT" && (next == "OSS" || next.starts_with(|c: char| c.is_ascii_digit())))
        || (previous == "OSS" && is_parameter_size(next))
        || (previous == "Flash" && next == "Lite")
}

impl App {
    pub(super) fn select_model(&mut self, requested: &str) -> Result<()> {
        let (model_id, matches) = resolve_model_reference(&self.settings, requested);
        if matches.len() > 1 {
            self.model_choices = Some(
                matches
                    .iter()
                    .map(|(index, registered_id)| {
                        (
                            *index,
                            self.settings.providers[*index].name.clone(),
                            registered_id.clone(),
                        )
                    })
                    .collect(),
            );
            self.model_choice_index = 0;
            self.pending_model = Some(model_id.clone());
            return Ok(());
        }
        if let Some((index, registered_id)) = matches.first() {
            self.activate_model(*index, registered_id)?;
            if !registered_id.eq_ignore_ascii_case(requested) {
                self.notice = format!(
                    "`{requested}` matched {registered_id} via {}.",
                    self.settings.providers[*index].name
                );
            }
            return Ok(());
        }

        let configured_elsewhere = find_model_matches_all(&self.settings, &model_id);
        if !configured_elsewhere.is_empty() {
            let providers = configured_elsewhere
                .iter()
                .map(|(index, _)| self.settings.providers[*index].name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            self.notice = format!(
                "Model is configured in {providers}, but no matching provider is active for automatic switching. Select one and press Space to activate it."
            );
            return Ok(());
        }
        // Unlisted model IDs are forced through the configured default provider.
        let fallback_id = self
            .settings
            .default_provider_id
            .as_deref()
            .or(self.settings.active_provider_id.as_deref());
        let fallback = fallback_id.and_then(|id| {
            self.settings
                .providers
                .iter()
                .position(|profile| profile.id == id && !profile.draft)
        });
        if let Some(index) = fallback {
            self.activate_model(index, &model_id)
        } else if self.settings.provider.is_some() {
            self.settings.model = Some(model_id.clone());
            write_settings(&self.settings)?;
            self.notice = format!("Model override set to {model_id}.");
            Ok(())
        } else {
            self.notice = format!(
                "No enabled provider has model `{model_id}`. Add it to a provider or activate a provider before forcing an unlisted model."
            );
            Ok(())
        }
    }

    pub(super) fn activate_model(&mut self, provider_index: usize, model_id: &str) -> Result<()> {
        let profile = &self.settings.providers[provider_index];
        let provider_id = profile.id.clone();
        let provider_name = profile.name.clone();
        let adapter = profile.adapter.clone();
        let base_url = profile.base_url.clone();
        self.settings.active_provider_id = Some(provider_id.clone());
        self.settings.provider = Some(adapter);
        self.settings.base_url = base_url;
        self.settings.api_key_env = None;
        self.settings.model = Some(model_id.to_owned());
        let currently_active = self.settings.active_chain_id.as_deref().and_then(|active| {
            self.settings
                .model_chains
                .iter()
                .find(|chain| chain.id == active)
        });
        self.settings.active_chain_id = currently_active
            .filter(|chain| {
                chain.members.iter().any(|member| {
                    member.provider_id == provider_id
                        && member.model_id.eq_ignore_ascii_case(model_id)
                })
            })
            .map(|chain| chain.id.clone())
            .or_else(|| {
                self.settings
                    .model_chains
                    .iter()
                    .find(|chain| {
                        chain.activate_on_select
                            && chain.members.iter().any(|member| {
                                member.provider_id == provider_id
                                    && member.model_id.eq_ignore_ascii_case(model_id)
                            })
                    })
                    .map(|chain| chain.id.clone())
            });
        self.provider_index = provider_index;
        self.refresh_usage_warning();
        write_settings(&self.settings)?;
        self.model_choices = None;
        self.pending_model = None;
        self.notice = format!("Model set to {model_id} via {provider_name}.");
        Ok(())
    }

    pub(super) fn activate_chain(&mut self, chain_id: &str) -> Result<()> {
        let chain = self
            .settings
            .model_chains
            .iter()
            .find(|chain| {
                chain.id.eq_ignore_ascii_case(chain_id)
                    || chain.alias.eq_ignore_ascii_case(chain_id)
            })
            .cloned();
        let Some(chain) = chain else {
            self.notice = format!(
                "No model chain named `{chain_id}` exists. Use /settings → Auto-switch models to create one."
            );
            return Ok(());
        };
        let Some(member) = chain.members.first() else {
            self.notice = format!("Chain `{}` has no models configured.", chain.id);
            return Ok(());
        };
        let Some(profile) = self
            .settings
            .providers
            .iter()
            .find(|profile| profile.id == member.provider_id && !profile.draft)
            .cloned()
        else {
            self.notice = format!(
                "Chain `{}` refers to a provider that is no longer enabled.",
                chain.id
            );
            return Ok(());
        };
        self.settings.active_chain_id = Some(chain.id.clone());
        self.settings.active_provider_id = Some(profile.id.clone());
        self.settings.provider = Some(profile.adapter.clone());
        self.settings.base_url = profile.base_url.clone();
        self.settings.model = Some(member.model_id.clone());
        self.settings.api_key_env = None;
        self.provider_index = self
            .settings
            .providers
            .iter()
            .position(|candidate| candidate.id == profile.id)
            .unwrap_or(0);
        write_settings(&self.settings)?;
        self.notice = format!(
            "Chain `{}` active; starting with {}.",
            chain.alias,
            selected_model_name(&self.settings, &member.model_id)
        );
        Ok(())
    }

    pub(super) fn toggle_chain(&mut self) -> Result<()> {
        if self.settings.active_chain_id.take().is_some() {
            write_settings(&self.settings)?;
            self.notice = "Model chain disabled; rate-limit errors will stop instead of switching."
                .to_owned();
            return Ok(());
        }
        let active_provider = self.settings.active_provider_id.as_deref();
        let active_model = self.settings.model.as_deref();
        let matching = self.settings.model_chains.iter().find(|chain| {
            chain.members.iter().any(|member| {
                Some(member.provider_id.as_str()) == active_provider
                    && Some(member.model_id.as_str()) == active_model
            })
        });
        if let Some(chain) = matching {
            self.settings.active_chain_id = Some(chain.id.clone());
            write_settings(&self.settings)?;
            self.notice = format!("Chain `{}` enabled.", chain.alias);
        } else {
            self.notice =
                "The current model is not in a chain. Use /chain <id> to choose one.".to_owned();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{model_author_matches, model_name, resolve_model_reference, selected_model_name};
    use crate::{ModelProfile, ProviderProfile, Settings};

    #[test]
    fn model_names_apply_series_styling_and_number_runs() {
        assert_eq!(model_name("claude-opus-5-5"), "Claude Opus 5.5");
        assert_eq!(model_name("gemini-3.8-flash"), "Gemini 3.8 Flash");
        assert_eq!(model_name("qwen3.8-max"), "Qwen3.8-Max");
        assert_eq!(model_name("glm-5.3-flash"), "GLM 5.3 Flash");
        assert_eq!(model_name("openai/gpt-oss-120b"), "GPT-OSS-120B");
        assert_eq!(model_name("gpt-6-astra"), "GPT-6 Astra");
        assert_eq!(model_name("gpt-6.1-sol"), "GPT-6.1 Sol");
        assert_eq!(model_name("gpt-6-1-sol"), "GPT-6.1 Sol");
        assert_eq!(
            model_name("gemini-flash-lite-latest"),
            "Gemini Flash-Lite Latest"
        );
        assert_eq!(model_name("gemini-flash-latest"), "Gemini Flash Latest");
        assert_eq!(model_name("llama-3-70b-instruct"), "Llama 3 70B Instruct");
    }

    #[test]
    fn name_parts_with_a_dot_are_capitalized_too() {
        assert_eq!(model_name("kimi-k2.7-code"), "Kimi K2.7 Code");
        assert_eq!(model_name("kimi-k3"), "Kimi K3");
        // Parts that start with a digit are untouched.
        assert_eq!(model_name("gemini-3.5-flash"), "Gemini 3.5 Flash");
        assert_eq!(model_name("some-v1.5-mini"), "Some V1.5 Mini");
    }

    #[test]
    fn model_author_namespace_is_not_a_provider_selector() {
        assert!(model_author_matches("anthropic", "claude-opus-5"));
        assert!(model_author_matches("openai", "gpt-6-luna"));
        assert!(!model_author_matches("anthropic", "gpt-6-luna"));
    }

    fn profile(id: &str, adapter: &str, models: &[&str], auto_switch: bool) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: id.to_owned(),
            adapter: adapter.to_owned(),
            model: models[0].to_owned(),
            models: models
                .iter()
                .map(|model| ModelProfile {
                    id: (*model).to_owned(),
                    name: String::new(),
                })
                .collect(),
            draft: false,
            auto_switch,
            base_url: None,
            ..Default::default()
        }
    }

    #[test]
    fn bare_model_name_resolves_to_provider_with_author_prefixed_id() {
        let mut settings = Settings::default();
        settings.default_provider_id = Some("google".to_owned());
        settings.providers = vec![
            profile("google", "google", &["gemini-flash-latest"], true),
            profile(
                "groq",
                "openai-compatible",
                &["qwen/qwen3.8-27b", "openai/gpt-oss-120b"],
                true,
            ),
        ];
        let (resolved, matches) = resolve_model_reference(&settings, "gpt-oss-120b");
        assert_eq!(resolved, "gpt-oss-120b");
        assert_eq!(matches, vec![(1, "openai/gpt-oss-120b".to_owned())]);
    }

    #[test]
    fn bare_model_name_is_not_guessed_when_a_profile_has_several_author_matches() {
        let mut settings = Settings::default();
        settings.providers = vec![profile(
            "router",
            "openai-compatible",
            &["openai/shared-model", "meta/shared-model"],
            true,
        )];
        let (_, matches) = resolve_model_reference(&settings, "shared-model");
        assert!(matches.is_empty());
    }

    #[test]
    fn a_series_name_resolves_to_the_newest_listed_model() {
        let mut settings = Settings::default();
        settings.providers = vec![profile(
            "multiai",
            "openai-compatible",
            &["claude-fable-5", "claude-fable-5-1", "claude-opus-5-5"],
            false,
        )];
        let (resolved, matches) = resolve_model_reference(&settings, "fable");
        assert_eq!(resolved, "claude-fable-5-1");
        assert_eq!(matches, vec![(0, "claude-fable-5-1".to_owned())]);
        let (_, exact) = resolve_model_reference(&settings, "fable-5");
        assert_eq!(exact, vec![(0, "claude-fable-5".to_owned())]);
        let (_, none) = resolve_model_reference(&settings, "banana");
        assert!(none.is_empty());
    }

    #[test]
    fn selecting_a_series_name_activates_the_match_and_says_so() {
        use crate::tui::state::App;
        let mut app = App::new(Settings::default());
        app.settings.providers = vec![profile(
            "multiai",
            "openai-compatible",
            &["claude-fable-5", "claude-fable-5-1"],
            false,
        )];
        app.select_model("fable").expect("select");
        assert_eq!(app.settings.model.as_deref(), Some("claude-fable-5-1"));
        assert!(app.notice.contains("fable"), "{}", app.notice);
        assert!(app.notice.contains("claude-fable-5-1"), "{}", app.notice);
    }

    #[test]
    fn exact_model_id_wins_over_author_suffix_match() {
        let mut settings = Settings::default();
        settings.providers = vec![profile(
            "router",
            "openai-compatible",
            &["openai/gpt-oss-120b", "gpt-oss-120b"],
            true,
        )];
        let (_, matches) = resolve_model_reference(&settings, "gpt-oss-120b");
        assert_eq!(matches, vec![(0, "gpt-oss-120b".to_owned())]);
    }

    #[test]
    fn author_qualified_model_resolves_across_multiple_provider_profiles() {
        let mut settings = Settings::default();
        settings.providers = vec![
            ProviderProfile {
                id: "router".to_owned(),
                name: "OpenRouter".to_owned(),
                adapter: "openai-compatible".to_owned(),
                model: "anthropic/claude-opus-5".to_owned(),
                models: vec![ModelProfile {
                    id: "anthropic/claude-opus-5".to_owned(),
                    name: String::new(),
                }],
                draft: false,
                auto_switch: true,
                base_url: None,
                ..Default::default()
            },
            ProviderProfile {
                id: "direct".to_owned(),
                name: "Claude API".to_owned(),
                adapter: "anthropic".to_owned(),
                model: "claude-opus-5".to_owned(),
                models: vec![ModelProfile {
                    id: "claude-opus-5".to_owned(),
                    name: String::new(),
                }],
                draft: false,
                auto_switch: true,
                base_url: None,
                ..Default::default()
            },
        ];
        let (resolved, matches) = resolve_model_reference(&settings, "anthropic/claude-opus-5");
        assert_eq!(resolved, "claude-opus-5");
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn auto_switch_provider_wins_over_default_provider_model_collision() {
        let mut settings = Settings::default();
        settings.default_provider_id = Some("google".to_owned());
        settings.providers = vec![
            ProviderProfile {
                id: "google".to_owned(),
                name: "Gemini default".to_owned(),
                adapter: "google".to_owned(),
                model: "gpt-oss-120b".to_owned(),
                models: vec![ModelProfile {
                    id: "gpt-oss-120b".to_owned(),
                    name: "Wrong provider".to_owned(),
                }],
                draft: false,
                auto_switch: false,
                base_url: None,
                ..Default::default()
            },
            ProviderProfile {
                id: "groq".to_owned(),
                name: "Groq auto".to_owned(),
                adapter: "groq".to_owned(),
                model: "gpt-oss-120b".to_owned(),
                models: vec![ModelProfile {
                    id: "gpt-oss-120b".to_owned(),
                    name: "GPT OSS".to_owned(),
                }],
                draft: false,
                auto_switch: true,
                base_url: None,
                ..Default::default()
            },
        ];
        let (_, matches) = resolve_model_reference(&settings, "gpt-oss-120b");
        assert_eq!(matches.len(), 1);
        assert_eq!(settings.providers[matches[0].0].id, "groq");
    }

    #[test]
    fn status_uses_configured_model_display_name() {
        let mut settings = Settings::default();
        settings.model = Some("claude-opus-5".to_owned());
        settings.active_provider_id = Some("test".to_owned());
        settings.providers.push(ProviderProfile {
            id: "test".to_owned(),
            name: "Main".to_owned(),
            adapter: "anthropic".to_owned(),
            model: "claude-opus-5".to_owned(),
            models: vec![ModelProfile {
                id: "claude-opus-5".to_owned(),
                name: "Opus latest".to_owned(),
            }],
            draft: false,
            auto_switch: false,
            base_url: None,
            ..Default::default()
        });
        assert_eq!(
            selected_model_name(&settings, "claude-opus-5"),
            "Opus latest"
        );
    }
}
