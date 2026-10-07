//! Telling the user before a provider runs dry.
//!
//! Providers with a limits endpoint report either a pay-as-you-go balance (tokens left) or
//! subscription windows (a fraction left). This module turns those lines into warnings; the app
//! announces each one once and keeps a short note in the status line while it applies.

use crate::endpoints::LimitLine;
use crate::tui::settings::sync::LimitsState;
use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
use std::collections::HashSet;

/// Warn when this fraction of a subscription window (or less) is left.
const WINDOW_LOW: f32 = 0.20;
/// Warn loudly at this fraction.
const WINDOW_CRITICAL: f32 = 0.05;
/// A balance under this many tokens is low.
const BALANCE_LOW: u64 = 100_000;
const BALANCE_CRITICAL: u64 = 20_000;
/// With a large starting balance, also warn when only this share of the largest balance seen is
/// left, and only when that peak was at least this big.
const BALANCE_SHARE_LOW: f64 = 0.10;
const BALANCE_SHARE_MIN_PEAK: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::tui) enum Severity {
    Low,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct UsageWarning {
    /// Stable per provider, line and severity, so each is announced only once.
    pub(in crate::tui) key: String,
    pub(in crate::tui) severity: Severity,
    /// The full sentence.
    pub(in crate::tui) message: String,
    /// A few words for the status line.
    pub(in crate::tui) short: String,
}

fn percent(fraction: f32) -> u32 {
    (fraction * 100.0).round().clamp(0.0, 100.0) as u32
}

/// "1.2M", "85k", "340": a token count in a few characters.
pub(in crate::tui) fn compact_tokens(tokens: u64) -> String {
    match tokens {
        1_000_000.. => format!("{:.1}M", tokens as f64 / 1_000_000.0),
        10_000.. => format!("{}k", tokens / 1_000),
        1_000.. => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => tokens.to_string(),
    }
}

/// The warnings that apply to `lines` right now. `peak_balance` remembers the largest balance
/// seen for this provider, to judge a shrinking balance against.
pub(in crate::tui) fn evaluate(
    provider: &str,
    lines: &[LimitLine],
    peak_balance: &mut u64,
) -> Vec<UsageWarning> {
    let mut warnings = Vec::new();
    for line in lines {
        if let Some(tokens) = line.balance_tokens {
            *peak_balance = (*peak_balance).max(tokens);
            let share_low = *peak_balance >= BALANCE_SHARE_MIN_PEAK
                && (tokens as f64) < *peak_balance as f64 * BALANCE_SHARE_LOW;
            let severity = if tokens == 0 || tokens < BALANCE_CRITICAL {
                Some(Severity::Critical)
            } else if tokens < BALANCE_LOW || share_low {
                Some(Severity::Low)
            } else {
                None
            };
            if let Some(severity) = severity {
                let (message, short) = if tokens == 0 {
                    (
                        format!("{provider} is out of tokens."),
                        "out of tokens".to_owned(),
                    )
                } else {
                    (
                        format!(
                            "{provider} is running low on tokens: about {} left.",
                            compact_tokens(tokens)
                        ),
                        format!("{} tokens left", compact_tokens(tokens)),
                    )
                };
                warnings.push(UsageWarning {
                    key: format!("{provider}:balance:{severity:?}"),
                    severity,
                    message,
                    short,
                });
            }
        } else if let Some(remaining) = line.remaining {
            let severity = if remaining <= WINDOW_CRITICAL {
                Some(Severity::Critical)
            } else if remaining <= WINDOW_LOW {
                Some(Severity::Low)
            } else {
                None
            };
            if let Some(severity) = severity {
                let left = percent(remaining);
                let (message, short) = if remaining <= 0.0 {
                    (
                        format!("{provider} has reached its {} limit.", line.label),
                        format!("{} limit reached", line.label),
                    )
                } else if severity == Severity::Critical {
                    (
                        format!(
                            "{provider} is almost at its {} limit: {left}% left.",
                            line.label
                        ),
                        format!("{} {left}% left", line.label),
                    )
                } else {
                    (
                        format!(
                            "{provider} is approaching its {} limit: {left}% left.",
                            line.label
                        ),
                        format!("{} {left}% left", line.label),
                    )
                };
                warnings.push(UsageWarning {
                    key: format!("{provider}:{}:{severity:?}", line.label),
                    severity,
                    message,
                    short,
                });
            }
        }
    }
    warnings
}

/// The most serious of `warnings` (the first when equally serious).
pub(in crate::tui) fn worst(warnings: &[UsageWarning]) -> Option<&UsageWarning> {
    warnings
        .iter()
        .fold(None, |best: Option<&UsageWarning>, candidate| match best {
            Some(current) if current.severity >= candidate.severity => Some(current),
            _ => Some(candidate),
        })
}

impl App {
    /// Index of the active provider, when it has a limits endpoint worth watching.
    fn watched_provider(&self) -> Option<usize> {
        let id = self.settings.active_provider_id.as_deref()?;
        let index = self
            .settings
            .providers
            .iter()
            .position(|profile| profile.id == id)?;
        self.settings.providers[index]
            .limits_url
            .is_some()
            .then_some(index)
    }

    /// Asks the active provider for its usage again (a fresh answer is reused for a minute).
    /// Called after each finished turn, because that provider is being used anyway.
    pub(in crate::tui) fn check_usage_after_turn(&mut self) {
        if !self.settings.usage_warnings {
            return;
        }
        if let Some(index) = self.watched_provider() {
            self.start_limits_fetch(index, false);
        }
    }

    /// Looks at fresh limits for `provider_id` and warns if they are running low.
    pub(in crate::tui) fn check_usage(&mut self, provider_id: &str, lines: &[LimitLine]) {
        let active = self.settings.active_provider_id.as_deref() == Some(provider_id);
        if !self.settings.usage_warnings || !active {
            if active {
                self.usage_warning = None;
            }
            return;
        }
        let name = self
            .settings
            .providers
            .iter()
            .find(|profile| profile.id == provider_id)
            .map_or_else(|| provider_id.to_owned(), |profile| profile.name.clone());
        let peak = self
            .peak_balances
            .entry(provider_id.to_owned())
            .or_insert(0);
        let warnings = evaluate(&name, lines, peak);
        // Warnings that no longer apply are forgotten, so they are announced again if they return.
        let current = warnings
            .iter()
            .map(|warning| warning.key.clone())
            .collect::<HashSet<_>>();
        let prefix = format!("{name}:");
        self.announced_warnings
            .retain(|key| !key.starts_with(&prefix) || current.contains(key));
        for warning in &warnings {
            if self.announced_warnings.insert(warning.key.clone()) {
                self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::CommandOutput,
                    text: format!("!! {} !!", warning.message),
                });
                self.history_scroll = 0;
                self.notice = warning.message.clone();
            }
        }
        self.usage_warning = worst(&warnings).cloned();
    }

    /// Recomputes the status-line warning from what is already known about the active provider
    /// (used when the provider changes).
    pub(in crate::tui) fn refresh_usage_warning(&mut self) {
        let Some(id) = self.settings.active_provider_id.clone() else {
            self.usage_warning = None;
            return;
        };
        let lines = match self.limits.get(&id).map(|entry| &entry.state) {
            Some(LimitsState::Ready(lines)) => lines.clone(),
            _ => {
                self.usage_warning = None;
                return;
            }
        };
        // Switching back to a provider must not repeat what was already announced.
        self.check_usage(&id, &lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, remaining: f32) -> LimitLine {
        LimitLine {
            label: label.to_owned(),
            value: String::new(),
            remaining: Some(remaining),
            balance_tokens: None,
        }
    }

    fn balance(tokens: u64) -> LimitLine {
        LimitLine {
            label: "Balance".to_owned(),
            value: String::new(),
            remaining: None,
            balance_tokens: Some(tokens),
        }
    }

    fn check(lines: &[LimitLine]) -> Vec<UsageWarning> {
        evaluate("MultiAI", lines, &mut 0)
    }

    #[test]
    fn plenty_left_means_no_warning() {
        assert!(check(&[window("5-hour", 0.8), balance(3_000_000)]).is_empty());
        assert!(check(&[]).is_empty());
        let plain = LimitLine {
            label: "Plan".to_owned(),
            value: "active".to_owned(),
            remaining: None,
            balance_tokens: None,
        };
        assert!(
            check(&[plain]).is_empty(),
            "lines without numbers cannot warn"
        );
    }

    #[test]
    fn a_subscription_window_warns_as_it_runs_down() {
        let low = check(&[window("5-hour", 0.15)]);
        assert_eq!(low.len(), 1);
        assert_eq!(low[0].severity, Severity::Low);
        assert!(
            low[0]
                .message
                .contains("approaching its 5-hour limit: 15% left"),
            "{}",
            low[0].message
        );
        assert_eq!(low[0].short, "5-hour 15% left");
        let critical = check(&[window("Weekly", 0.04)]);
        assert_eq!(critical[0].severity, Severity::Critical);
        assert!(
            critical[0].message.contains("almost at its Weekly limit"),
            "{}",
            critical[0].message
        );
        let spent = check(&[window("Monthly", 0.0)]);
        assert_eq!(spent[0].severity, Severity::Critical);
        assert!(
            spent[0].message.contains("has reached its Monthly limit"),
            "{}",
            spent[0].message
        );
        assert_eq!(
            check(&[window("5-hour", 0.20)]).len(),
            1,
            "exactly 20% counts"
        );
        assert!(check(&[window("5-hour", 0.21)]).is_empty());
    }

    #[test]
    fn a_pay_as_you_go_balance_warns_in_tokens() {
        let low = check(&[balance(85_000)]);
        assert_eq!(low[0].severity, Severity::Low);
        assert!(
            low[0]
                .message
                .contains("running low on tokens: about 85k left"),
            "{}",
            low[0].message
        );
        assert_eq!(low[0].short, "85k tokens left");
        assert_eq!(check(&[balance(15_000)])[0].severity, Severity::Critical);
        let empty = check(&[balance(0)]);
        assert_eq!(empty[0].severity, Severity::Critical);
        assert!(
            empty[0].message.contains("out of tokens"),
            "{}",
            empty[0].message
        );
        assert!(check(&[balance(100_000)]).is_empty());
    }

    #[test]
    fn a_big_balance_also_warns_by_share_of_its_peak() {
        let mut peak = 0;
        assert!(evaluate("MultiAI", &[balance(3_000_000)], &mut peak).is_empty());
        assert_eq!(peak, 3_000_000);
        let draining = evaluate("MultiAI", &[balance(250_000)], &mut peak);
        assert_eq!(
            draining.len(),
            1,
            "under 10% of 3M is low although 250k is not tiny"
        );
        assert_eq!(draining[0].severity, Severity::Low);
        let mut small_peak = 0;
        evaluate("MultiAI", &[balance(500_000)], &mut small_peak);
        assert!(evaluate("MultiAI", &[balance(120_000)], &mut small_peak).is_empty());
    }

    #[test]
    fn keys_are_stable_per_line_and_severity() {
        let a = check(&[window("5-hour", 0.15)]);
        let b = check(&[window("5-hour", 0.12)]);
        assert_eq!(
            a[0].key, b[0].key,
            "the same warning is not announced twice"
        );
        let worse = check(&[window("5-hour", 0.03)]);
        assert_ne!(a[0].key, worse[0].key, "getting worse is announced again");
        let other_line = check(&[window("Weekly", 0.15)]);
        assert_ne!(a[0].key, other_line[0].key);
    }

    #[test]
    fn the_worst_warning_wins_for_the_status_line() {
        let warnings = check(&[
            window("5-hour", 0.15),
            window("Weekly", 0.02),
            balance(90_000),
        ]);
        assert_eq!(warnings.len(), 3);
        let worst = worst(&warnings).expect("one");
        assert_eq!(worst.severity, Severity::Critical);
        assert_eq!(worst.short, "Weekly 2% left");
        assert!(super::worst(&[]).is_none());
    }

    #[test]
    fn token_counts_are_compact() {
        for (tokens, text) in [
            (0, "0"),
            (340, "340"),
            (1_500, "1.5k"),
            (85_000, "85k"),
            (2_400_000, "2.4M"),
        ] {
            assert_eq!(compact_tokens(tokens), text);
        }
    }

    // ---- the app side: announcing, status line, and when limits are re-checked ----

    use crate::tui::settings::sync::{LimitsEntry, TaskResult};
    use crate::tui::state::App;
    use crate::{ProviderProfile, Settings};

    fn provider(id: &str, name: &str, limits: bool) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: name.to_owned(),
            adapter: "openai-compatible".to_owned(),
            base_url: Some("https://api.example.com/v1".to_owned()),
            limits_url: limits.then(|| "https://api.example.com/v1/limits".to_owned()),
            model: "m".to_owned(),
            ..Default::default()
        }
    }

    fn app() -> App {
        let mut settings = Settings::default();
        settings.providers = vec![
            provider("multiai", "MultiAI", true),
            provider("other", "Other", true),
        ];
        settings.active_provider_id = Some("multiai".to_owned());
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app
    }

    fn deliver(app: &mut App, provider_id: &str, lines: Vec<LimitLine>) {
        app.apply_task_result(TaskResult::Limits {
            provider_id: provider_id.to_owned(),
            url: "https://api.example.com/v1/limits".to_owned(),
            result: Ok(lines),
        });
    }

    fn warnings_in(app: &App) -> usize {
        app.transcript
            .iter()
            .filter(|entry| entry.text.starts_with("!! "))
            .count()
    }

    #[test]
    fn a_low_window_is_announced_once_and_stays_in_the_status_line() {
        let mut app = app();
        deliver(&mut app, "multiai", vec![window("5-hour", 0.15)]);
        assert_eq!(warnings_in(&app), 1);
        assert!(
            app.notice.contains("approaching its 5-hour limit"),
            "{}",
            app.notice
        );
        let shown = app.usage_warning.clone().expect("status warning");
        assert_eq!(shown.short, "5-hour 15% left");
        // The same warning on the next check is not said again, but still shown.
        app.notice.clear();
        deliver(&mut app, "multiai", vec![window("5-hour", 0.12)]);
        assert_eq!(warnings_in(&app), 1, "no repeat");
        assert!(app.notice.is_empty());
        assert_eq!(
            app.usage_warning.as_ref().map(|w| w.short.as_str()),
            Some("5-hour 12% left")
        );
    }

    #[test]
    fn getting_worse_is_announced_again_and_recovery_clears_the_status() {
        let mut app = app();
        deliver(&mut app, "multiai", vec![window("5-hour", 0.15)]);
        deliver(&mut app, "multiai", vec![window("5-hour", 0.03)]);
        assert_eq!(warnings_in(&app), 2, "low, then critical");
        assert!(
            app.notice.contains("almost at its 5-hour limit"),
            "{}",
            app.notice
        );
        deliver(&mut app, "multiai", vec![window("5-hour", 0.9)]);
        assert!(app.usage_warning.is_none(), "the window reset");
        deliver(&mut app, "multiai", vec![window("5-hour", 0.1)]);
        assert_eq!(
            warnings_in(&app),
            3,
            "a returning warning is announced again"
        );
    }

    #[test]
    fn a_shrinking_balance_warns_in_tokens() {
        let mut app = app();
        deliver(&mut app, "multiai", vec![balance(3_000_000)]);
        assert!(app.usage_warning.is_none());
        deliver(&mut app, "multiai", vec![balance(90_000)]);
        assert!(
            app.notice.contains("running low on tokens"),
            "{}",
            app.notice
        );
        assert_eq!(
            app.usage_warning.as_ref().map(|w| w.short.as_str()),
            Some("90k tokens left")
        );
    }

    #[test]
    fn only_the_active_provider_is_warned_about() {
        let mut app = app();
        deliver(&mut app, "other", vec![window("5-hour", 0.01)]);
        assert_eq!(warnings_in(&app), 0);
        assert!(app.usage_warning.is_none());
    }

    #[test]
    fn the_switch_in_settings_turns_everything_off() {
        let mut app = app();
        app.settings.usage_warnings = false;
        deliver(&mut app, "multiai", vec![window("5-hour", 0.01)]);
        assert_eq!(warnings_in(&app), 0);
        assert!(app.usage_warning.is_none());
        app.check_usage_after_turn();
        assert_eq!(
            app.spawned_tasks, 0,
            "not even a request when warnings are off"
        );
    }

    #[test]
    fn switching_provider_shows_that_providers_known_state_without_repeating() {
        let mut app = app();
        app.settings.providers[1].models = vec![crate::ModelProfile {
            id: "m".to_owned(),
            name: String::new(),
        }];
        app.limits.insert(
            "other".to_owned(),
            LimitsEntry {
                fetched_at: std::time::Instant::now(),
                state: LimitsState::Ready(vec![window("Weekly", 0.04)]),
            },
        );
        assert!(app.usage_warning.is_none());
        app.activate_model(1, "m").expect("switch");
        assert_eq!(
            app.usage_warning.as_ref().map(|w| w.short.as_str()),
            Some("Weekly 4% left")
        );
        let announced = warnings_in(&app);
        app.activate_model(0, "m").expect("switch back");
        assert!(app.usage_warning.is_none(), "MultiAI has nothing known");
        app.activate_model(1, "m").expect("and again");
        assert_eq!(warnings_in(&app), announced, "not announced a second time");
    }

    #[test]
    fn usage_is_rechecked_after_a_turn_only_for_a_provider_that_reports_it() {
        let mut app = app();
        app.check_usage_after_turn();
        assert_eq!(
            app.spawned_tasks, 1,
            "the active provider has a limits endpoint"
        );
        app.check_usage_after_turn();
        assert_eq!(app.spawned_tasks, 1, "a request is already under way");
        let mut quiet = App::new({
            let mut settings = Settings::default();
            settings.providers = vec![provider("plain", "Plain", false)];
            settings.active_provider_id = Some("plain".to_owned());
            settings
        });
        quiet.check_usage_after_turn();
        assert_eq!(quiet.spawned_tasks, 0, "no limits endpoint, nothing to ask");
    }

    #[test]
    fn the_status_line_shows_the_warning_in_the_house_style() {
        use crate::tui::render::draw;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut app = app();
        deliver(&mut app, "multiai", vec![window("5-hour", 0.15)]);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("!! 5-hour 15% left !!"), "{text}");
        assert!(!text.contains('⚠'), "the old warning sign is not used");
        app.usage_warning = None;
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        let clear = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            !clear.contains("!! 5-hour 15% left !!"),
            "the status marker goes away"
        );
    }
}
