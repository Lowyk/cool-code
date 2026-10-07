use crate::endpoints::{
    FetchedModel, LimitLine, fetch_json, merge_models, parse_models, summarize_limits,
};
use crate::tui::state::App;
use crate::write_settings;
use anyhow::Result;
use std::time::{Duration, Instant};

/// A limits response is reused for this long before selecting the provider fetches it again.
const LIMITS_FRESH_FOR: Duration = Duration::from_secs(60);

pub(in crate::tui) enum TaskResult {
    Models {
        provider_id: String,
        /// The endpoint that was fetched; the result is dropped if it has since changed.
        url: String,
        promote: bool,
        result: Result<Vec<FetchedModel>, String>,
    },
    Limits {
        provider_id: String,
        url: String,
        result: Result<Vec<LimitLine>, String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum LimitsState {
    Loading,
    Ready(Vec<LimitLine>),
    Failed(String),
}

pub(in crate::tui) struct LimitsEntry {
    pub(in crate::tui) fetched_at: Instant,
    pub(in crate::tui) state: LimitsState,
}

/// The key a fetch sends: the provider's credential-store entry, and only when the user
/// explicitly asked (`allow_env`) the global `HARNESS_API_KEY`. Automatic fetches never use the
/// environment key, which may belong to a different vendor.
#[cfg(not(test))]
fn load_key(provider_id: &str, allow_env: bool) -> Option<String> {
    crate::secrets::load(provider_id)
        .ok()
        .flatten()
        .or_else(|| {
            allow_env
                .then(|| std::env::var("HARNESS_API_KEY").ok())
                .flatten()
        })
        .filter(|key| !key.trim().is_empty())
}

// Tests must never read the real credential store. A provider named `no-key` has no key, and
// one named `env-only` has only the environment key.
#[cfg(test)]
fn load_key(provider_id: &str, allow_env: bool) -> Option<String> {
    match provider_id {
        "no-key" => None,
        "env-only" => allow_env.then(|| "env-key".to_owned()),
        _ => Some("test-key".to_owned()),
    }
}

/// Stored endpoints are re-checked before every request: a hand-edited or imported config must
/// not be able to send the API key to another host.
fn endpoint_is_trusted(profile: &crate::ProviderProfile, url: &str) -> bool {
    let base = profile.base_url.as_deref().unwrap_or_default();
    crate::endpoints::resolve_endpoint(base, url).is_ok_and(|resolved| resolved == url)
}

impl App {
    fn spawn_task(&mut self, job: impl FnOnce() -> TaskResult + Send + 'static) {
        #[cfg(test)]
        {
            let _ = job;
            self.spawned_tasks += 1;
        }
        #[cfg(not(test))]
        {
            let sender = self.tasks.clone();
            std::thread::spawn(move || {
                let _ = sender.send(job());
            });
        }
    }

    /// Fetches the provider's model list in the background. With `promote`, a draft provider
    /// that now has models and a key becomes an enabled provider.
    pub(in crate::tui) fn start_models_fetch(&mut self, index: usize, promote: bool) {
        let Some(profile) = self.settings.providers.get(index) else {
            return;
        };
        let Some(url) = profile.models_url.clone() else {
            self.notice = format!("{} has no models endpoint.", profile.name);
            return;
        };
        let id = profile.id.clone();
        let name = profile.name.clone();
        if !endpoint_is_trusted(profile, &url) {
            self.notice = format!(
                "{name}: the models endpoint is not on this provider's host, so it was not contacted."
            );
            return;
        }
        // A save-triggered fetch (promote) is automatic; only a manual refresh may use the env key.
        let Some(key) = load_key(&id, !promote) else {
            self.notice = format!("Add an API key for {name} to load its models.");
            return;
        };
        if !self.models_loading.insert(id.clone()) {
            return;
        }
        self.notice = format!("Loading models for {name}…");
        self.spawn_task(move || TaskResult::Models {
            provider_id: id,
            url: url.clone(),
            promote,
            result: fetch_json(&url, &key)
                .map(|value| parse_models(&value))
                .map_err(|error| format!("{error:#}")),
        });
    }

    /// Fetches usage limits in the background; a fresh cached answer is reused unless `force`.
    pub(in crate::tui) fn start_limits_fetch(&mut self, index: usize, force: bool) {
        let Some(profile) = self.settings.providers.get(index) else {
            return;
        };
        let Some(url) = profile.limits_url.clone() else {
            return;
        };
        let id = profile.id.clone();
        if let Some(entry) = self.limits.get(&id) {
            let fresh = entry.fetched_at.elapsed() < LIMITS_FRESH_FOR;
            if entry.state == LimitsState::Loading || (fresh && !force) {
                return;
            }
        }
        if !endpoint_is_trusted(profile, &url) {
            self.limits.insert(
                id,
                LimitsEntry {
                    fetched_at: Instant::now(),
                    state: LimitsState::Failed(
                        "the limits endpoint is not on this provider's host".to_owned(),
                    ),
                },
            );
            return;
        }
        // Loading usage when a provider is selected is automatic; only a forced refresh (u) may
        // use the env key.
        let Some(key) = load_key(&id, force) else {
            self.limits.insert(
                id,
                LimitsEntry {
                    fetched_at: Instant::now(),
                    state: LimitsState::Failed("add an API key to see usage".to_owned()),
                },
            );
            return;
        };
        self.limits.insert(
            id.clone(),
            LimitsEntry {
                fetched_at: Instant::now(),
                state: LimitsState::Loading,
            },
        );
        self.spawn_task(move || TaskResult::Limits {
            provider_id: id,
            url: url.clone(),
            result: fetch_json(&url, &key)
                .map(|value| summarize_limits(&value))
                .map_err(|error| format!("{error:#}")),
        });
    }

    /// Applies finished background fetches; called once per frame.
    pub(in crate::tui) fn poll_tasks(&mut self) {
        while let Ok(result) = self.task_results.try_recv() {
            self.apply_task_result(result);
        }
    }

    pub(in crate::tui) fn apply_task_result(&mut self, result: TaskResult) {
        match result {
            TaskResult::Limits {
                provider_id,
                url,
                result,
            } => {
                let current = self
                    .settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == provider_id)
                    .and_then(|profile| profile.limits_url.as_deref());
                if current != Some(url.as_str()) {
                    return;
                }
                let state = match result {
                    Ok(lines) if lines.is_empty() => {
                        LimitsState::Failed("the endpoint returned nothing to show".to_owned())
                    }
                    Ok(lines) => LimitsState::Ready(lines),
                    Err(error) => LimitsState::Failed(error),
                };
                self.limits.insert(
                    provider_id,
                    LimitsEntry {
                        fetched_at: Instant::now(),
                        state,
                    },
                );
            }
            TaskResult::Models {
                provider_id,
                url,
                promote,
                result,
            } => {
                self.models_loading.remove(&provider_id);
                let current = self
                    .settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == provider_id)
                    .and_then(|profile| profile.models_url.as_deref());
                if current != Some(url.as_str()) {
                    return;
                }
                // Only a stored key counts: the env key must never enable a provider.
                let has_key = load_key(&provider_id, false).is_some();
                self.apply_models_result(&provider_id, promote, has_key, result);
            }
        }
    }

    fn apply_models_result(
        &mut self,
        provider_id: &str,
        promote: bool,
        has_key: bool,
        result: Result<Vec<FetchedModel>, String>,
    ) {
        let Some(index) = self
            .settings
            .providers
            .iter()
            .position(|profile| profile.id == provider_id)
        else {
            return;
        };
        let name = self.settings.providers[index].name.clone();
        let fetched = match result {
            Ok(fetched) if !fetched.is_empty() => fetched,
            Ok(_) => {
                self.notice = format!("{name}: the models endpoint listed no text models.");
                return;
            }
            Err(error) => {
                self.notice = format!("{name}: could not load models: {error}");
                return;
            }
        };
        let profile = &mut self.settings.providers[index];
        let added = merge_models(profile, &fetched);
        let total = profile.models.len();
        let enabled = promote && profile.draft && has_key && total > 0;
        if enabled {
            profile.draft = false;
        }
        if let Err(error) = write_settings(&self.settings) {
            self.notice =
                format!("{name}: models loaded but settings could not be saved: {error:#}");
            return;
        }
        self.notice = match (enabled, added) {
            (true, _) => format!("{name}: {total} models loaded and the provider is enabled."),
            (false, 0) => format!("{name}: models are up to date ({total})."),
            (false, added) => format!("{name}: {added} new models loaded ({total} in total)."),
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::endpoints::{FetchedModel, LimitLine};
    use crate::tui::settings::sync::{LimitsEntry, LimitsState, TaskResult};
    use crate::tui::state::App;
    use crate::{ProviderProfile, Settings};
    use std::time::{Duration, Instant};

    fn provider(id: &str, draft: bool) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: format!("Provider {id}"),
            adapter: "openai-compatible".to_owned(),
            base_url: Some("https://api.example.com/v1".to_owned()),
            models_url: Some("https://api.example.com/v1/models".to_owned()),
            limits_url: Some("https://api.example.com/v1/limits".to_owned()),
            draft,
            ..Default::default()
        }
    }

    fn app_with(providers: Vec<ProviderProfile>) -> App {
        let mut settings = Settings::default();
        settings.providers = providers;
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app
    }

    fn fetched(ids: &[&str]) -> Vec<FetchedModel> {
        ids.iter()
            .map(|id| FetchedModel {
                id: (*id).to_owned(),
                name: String::new(),
                free: None,
                tools: Some(true),
                context: None,
            })
            .collect()
    }

    #[test]
    fn fetched_models_enable_a_draft_provider_that_has_a_key() {
        let mut app = app_with(vec![provider("p1", true)]);
        app.apply_models_result("p1", true, true, Ok(fetched(&["a", "b"])));
        let profile = &app.settings.providers[0];
        assert!(!profile.draft);
        assert_eq!(profile.models.len(), 2);
        assert_eq!(profile.model, "a");
        assert!(
            app.notice
                .contains("2 models loaded and the provider is enabled"),
            "{}",
            app.notice
        );
    }

    #[test]
    fn a_draft_without_a_key_stays_a_draft_but_keeps_its_models() {
        let mut app = app_with(vec![provider("p1", true)]);
        app.apply_models_result("p1", true, false, Ok(fetched(&["a"])));
        assert!(app.settings.providers[0].draft);
        assert_eq!(app.settings.providers[0].models.len(), 1);
    }

    #[test]
    fn a_manual_refresh_never_changes_draft_status() {
        let mut app = app_with(vec![provider("p1", true)]);
        app.apply_models_result("p1", false, true, Ok(fetched(&["a"])));
        assert!(app.settings.providers[0].draft);
    }

    #[test]
    fn failures_and_empty_lists_leave_the_provider_untouched() {
        let mut app = app_with(vec![provider("p1", false)]);
        app.apply_models_result("p1", true, true, Err("provider returned 401".to_owned()));
        assert!(
            app.notice.contains("could not load models"),
            "{}",
            app.notice
        );
        app.apply_models_result("p1", true, true, Ok(Vec::new()));
        assert!(app.notice.contains("no text models"), "{}", app.notice);
        assert!(app.settings.providers[0].models.is_empty());
    }

    #[test]
    fn results_for_a_deleted_provider_are_ignored() {
        let mut app = app_with(vec![provider("p1", false)]);
        app.apply_models_result("gone", true, true, Ok(fetched(&["a"])));
        assert!(app.settings.providers[0].models.is_empty());
    }

    #[test]
    fn starting_a_fetch_needs_an_endpoint_and_a_key_and_never_doubles_up() {
        let mut app = app_with(vec![provider("p1", false), provider("no-key", false)]);
        let mut plain = provider("p3", false);
        plain.models_url = None;
        app.settings.providers.push(plain);
        app.start_models_fetch(2, false);
        assert!(app.notice.contains("no models endpoint"), "{}", app.notice);
        app.start_models_fetch(1, false);
        assert!(app.notice.contains("Add an API key"), "{}", app.notice);
        assert_eq!(app.spawned_tasks, 0);
        app.start_models_fetch(0, false);
        app.start_models_fetch(0, false);
        assert_eq!(app.spawned_tasks, 1);
    }

    #[test]
    fn limits_are_cached_for_a_minute_and_force_refreshes() {
        let mut app = app_with(vec![provider("p1", false)]);
        app.start_limits_fetch(0, false);
        assert_eq!(app.spawned_tasks, 1);
        assert_eq!(app.limits["p1"].state, LimitsState::Loading);
        app.apply_task_result(TaskResult::Limits {
            provider_id: "p1".to_owned(),
            url: "https://api.example.com/v1/limits".to_owned(),
            result: Ok(vec![LimitLine {
                label: "Balance".to_owned(),
                value: "5 tokens".to_owned(),
                remaining: None,
            }]),
        });
        assert!(matches!(app.limits["p1"].state, LimitsState::Ready(_)));
        app.start_limits_fetch(0, false);
        assert_eq!(app.spawned_tasks, 1, "fresh cache is reused");
        app.start_limits_fetch(0, true);
        assert_eq!(app.spawned_tasks, 2, "force refetches");
        app.limits.insert(
            "p1".to_owned(),
            LimitsEntry {
                fetched_at: Instant::now() - Duration::from_secs(120),
                state: LimitsState::Ready(Vec::new()),
            },
        );
        app.start_limits_fetch(0, false);
        assert_eq!(app.spawned_tasks, 3, "stale cache refetches");
    }

    #[test]
    fn stored_endpoints_on_another_host_are_never_fetched() {
        let mut bad = provider("p1", false);
        bad.models_url = Some("http://evil.example/models".to_owned());
        bad.limits_url = Some("https://evil.example/limits".to_owned());
        let mut app = app_with(vec![bad]);
        app.start_models_fetch(0, false);
        assert!(
            app.notice.contains("not on this provider's host"),
            "{}",
            app.notice
        );
        app.start_limits_fetch(0, true);
        assert_eq!(app.spawned_tasks, 0);
        assert!(matches!(app.limits["p1"].state, LimitsState::Failed(_)));
    }

    #[test]
    fn automatic_fetches_never_use_the_environment_key() {
        let mut app = app_with(vec![provider("env-only", true)]);
        // A save-triggered (promoting) fetch and the on-select usage load are automatic.
        app.start_models_fetch(0, true);
        app.start_limits_fetch(0, false);
        assert_eq!(app.spawned_tasks, 0);
        assert!(app.notice.contains("Add an API key"), "{}", app.notice);
        // The user explicitly pressing f or u may use it.
        app.start_models_fetch(0, false);
        app.limits.clear();
        app.start_limits_fetch(0, true);
        assert_eq!(app.spawned_tasks, 2);
    }

    #[test]
    fn a_result_for_an_endpoint_that_changed_mid_fetch_is_ignored() {
        let mut app = app_with(vec![provider("p1", true)]);
        app.apply_task_result(TaskResult::Models {
            provider_id: "p1".to_owned(),
            url: "https://old.example.com/v1/models".to_owned(),
            promote: true,
            result: Ok(fetched(&["a"])),
        });
        assert!(app.settings.providers[0].models.is_empty());
        assert!(app.settings.providers[0].draft);
    }

    #[test]
    fn limits_errors_are_kept_for_display() {
        let mut app = app_with(vec![provider("p1", false)]);
        app.apply_task_result(TaskResult::Limits {
            provider_id: "p1".to_owned(),
            url: "https://api.example.com/v1/limits".to_owned(),
            result: Err("provider returned 401".to_owned()),
        });
        assert_eq!(
            app.limits["p1"].state,
            LimitsState::Failed("provider returned 401".to_owned())
        );
    }
}
