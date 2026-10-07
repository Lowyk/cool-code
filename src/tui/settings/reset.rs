//! Settings > General > Reset: put some or all of what the harness remembers back to scratch.
//!
//! Anything that deletes data asks for confirmation first, and says how much it will delete.

use crate::tui::settings::SettingsView;
use crate::tui::state::App;
use crate::{Settings, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum ResetTarget {
    TrustedFolders,
    Sessions,
    Providers,
    Settings,
    UsageStats,
    ProjectChoices,
    SetupWizard,
}

const ALL: [ResetTarget; 7] = [
    ResetTarget::TrustedFolders,
    ResetTarget::Sessions,
    ResetTarget::Providers,
    ResetTarget::Settings,
    ResetTarget::UsageStats,
    ResetTarget::ProjectChoices,
    ResetTarget::SetupWizard,
];

impl ResetTarget {
    fn label(self) -> &'static str {
        match self {
            ResetTarget::TrustedFolders => "Trusted folders",
            ResetTarget::Sessions => "Sessions",
            ResetTarget::Providers => "Providers (and their saved API keys)",
            ResetTarget::Settings => "Settings",
            ResetTarget::UsageStats => "Usage stats history",
            ResetTarget::ProjectChoices => "Per-project CLAUDE.md / AGENTS.md choices",
            ResetTarget::SetupWizard => "Setup wizard answers",
        }
    }
}

/// The menu entries, in order. `None` is the "pick your own" entry.
const MENU: [(&str, Option<&[ResetTarget]>); 7] = [
    ("Everything", Some(&ALL)),
    ("Trusted folders", Some(&[ResetTarget::TrustedFolders])),
    ("Sessions", Some(&[ResetTarget::Sessions])),
    ("Providers", Some(&[ResetTarget::Providers])),
    ("Settings", Some(&[ResetTarget::Settings])),
    ("Re-run Setup Wizard", Some(&[ResetTarget::SetupWizard])),
    ("Custom…", None),
];

#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum ResetStage {
    Menu { row: usize },
    Custom { row: usize, checks: [bool; 7] },
    Confirm { targets: Vec<ResetTarget> },
}

/// `settings` with the preferences back at their defaults. Providers, model choices and the
/// acknowledgements and answers the user already gave are kept: those have their own entries.
pub(in crate::tui) fn reset_preferences(settings: &Settings) -> Settings {
    let mut fresh = Settings::default();
    fresh.provider = settings.provider.clone();
    fresh.model = settings.model.clone();
    fresh.base_url = settings.base_url.clone();
    fresh.api_key_env = settings.api_key_env.clone();
    fresh.providers = settings.providers.clone();
    fresh.active_provider_id = settings.active_provider_id.clone();
    fresh.default_provider_id = settings.default_provider_id.clone();
    fresh.model_chains = settings.model_chains.clone();
    fresh.active_chain_id = settings.active_chain_id.clone();
    fresh.privacy_acknowledged = settings.privacy_acknowledged.clone();
    fresh.privacy_image_acknowledged = settings.privacy_image_acknowledged.clone();
    fresh.motion_prompt_answered = settings.motion_prompt_answered;
    fresh.stats_prompt_answered = settings.stats_prompt_answered;
    fresh.sessions_prompt_answered = settings.sessions_prompt_answered;
    fresh.theme_prompt_answered = settings.theme_prompt_answered;
    fresh.instructions_prompt_answered = settings.instructions_prompt_answered;
    fresh
}

impl App {
    /// A sentence describing what resetting `target` would remove right now.
    fn reset_effect(&self, target: ResetTarget) -> String {
        let plural = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        match target {
            ResetTarget::TrustedFolders => {
                let count = crate::projects::Registry::load_from(&self.projects_path).trusted_count();
                format!("forget {}", plural(count, "trusted folder", "trusted folders"))
            }
            ResetTarget::Sessions => {
                let count = crate::session::list_in(&self.session_dir, None).len();
                format!("delete {}", plural(count, "saved session", "saved sessions"))
            }
            ResetTarget::Providers => {
                let count = self.settings.providers.len();
                format!(
                    "remove {} and delete their API keys from the credential store",
                    plural(count, "provider", "providers")
                )
            }
            ResetTarget::Settings => "put theme, effort, mode, animations, the stats, session and instruction-file switches back to their defaults (providers and your earlier answers stay)".to_owned(),
            ResetTarget::UsageStats => "delete the usage-stats history".to_owned(),
            ResetTarget::ProjectChoices => {
                "forget the per-project CLAUDE.md / AGENTS.md choices".to_owned()
            }
            ResetTarget::SetupWizard => "ask the setup questions again".to_owned(),
        }
    }

    /// Carries out a reset and returns a one-line summary.
    pub(in crate::tui) fn apply_reset(&mut self, targets: &[ResetTarget]) -> Result<String> {
        let wants = |target: ResetTarget| targets.contains(&target);
        let mut done = Vec::new();
        if wants(ResetTarget::TrustedFolders) {
            crate::projects::update(
                &self.projects_path,
                crate::projects::Registry::clear_trusted,
            )?;
            self.workspace_trusted = false;
            self.trust_prompt = true;
            done.push("trusted folders");
        }
        if wants(ResetTarget::ProjectChoices) {
            crate::projects::update(&self.projects_path, |registry| {
                for entry in &mut registry.projects {
                    entry.load_claude_md = None;
                    entry.load_agents_md = None;
                }
            })?;
            done.push("project choices");
        }
        if wants(ResetTarget::Sessions) {
            for header in crate::session::list_in(&self.session_dir, None) {
                crate::session::delete_in(&self.session_dir, &header.id)?;
            }
            self.begin_new_session();
            done.push("sessions");
        }
        if wants(ResetTarget::UsageStats) {
            crate::stats::clear()?;
            done.push("usage stats");
        }
        if wants(ResetTarget::Providers) {
            for provider in &self.settings.providers {
                crate::secrets::delete(&provider.id)?;
            }
            self.settings.providers.clear();
            self.settings.model_chains.clear();
            self.settings.active_chain_id = None;
            self.settings.active_provider_id = None;
            self.settings.default_provider_id = None;
            self.settings.provider = Settings::default().provider;
            self.settings.model = None;
            self.settings.base_url = None;
            self.settings.api_key_env = None;
            self.provider_index = 0;
            done.push("providers");
        }
        if wants(ResetTarget::Settings) {
            self.settings = reset_preferences(&self.settings);
            done.push("settings");
        }
        if wants(ResetTarget::SetupWizard) {
            self.settings.theme_prompt_answered = false;
            self.settings.motion_prompt_answered = false;
            self.settings.stats_prompt_answered = false;
            self.settings.sessions_prompt_answered = false;
            self.settings.instructions_prompt_answered = false;
            done.push("the setup wizard");
        }
        write_settings(&self.settings)?;
        if wants(ResetTarget::SetupWizard) {
            self.start_setup();
        }
        Ok(format!("Reset: {}.", done.join(", ")))
    }

    pub(super) fn handle_reset_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(stage) = self
            .settings_view
            .as_ref()
            .and_then(|view| view.reset.clone())
        else {
            return Ok(());
        };
        let next = match stage {
            ResetStage::Menu { row } => match key.code {
                KeyCode::Up => Some(ResetStage::Menu {
                    row: row.saturating_sub(1),
                }),
                KeyCode::Down => Some(ResetStage::Menu {
                    row: (row + 1).min(MENU.len() - 1),
                }),
                KeyCode::Esc | KeyCode::Left => None,
                KeyCode::Enter => match MENU[row].1 {
                    None => Some(ResetStage::Custom {
                        row: 0,
                        checks: [false; 7],
                    }),
                    // Asking the questions again deletes nothing, so it needs no confirmation.
                    Some([ResetTarget::SetupWizard]) => {
                        let summary = self.apply_reset(&[ResetTarget::SetupWizard])?;
                        self.notice = summary;
                        self.settings_view = None;
                        return Ok(());
                    }
                    Some(targets) => Some(ResetStage::Confirm {
                        targets: targets.to_vec(),
                    }),
                },
                _ => Some(ResetStage::Menu { row }),
            },
            ResetStage::Custom { row, mut checks } => match key.code {
                KeyCode::Up => Some(ResetStage::Custom {
                    row: row.saturating_sub(1),
                    checks,
                }),
                KeyCode::Down => Some(ResetStage::Custom {
                    row: (row + 1).min(ALL.len() - 1),
                    checks,
                }),
                KeyCode::Char(' ') => {
                    checks[row] = !checks[row];
                    Some(ResetStage::Custom { row, checks })
                }
                KeyCode::Esc | KeyCode::Left => Some(ResetStage::Menu { row: 6 }),
                KeyCode::Enter => {
                    let targets = ALL
                        .iter()
                        .zip(checks)
                        .filter_map(|(target, ticked)| ticked.then_some(*target))
                        .collect::<Vec<_>>();
                    if targets.is_empty() {
                        self.notice = "Tick at least one thing to reset (Space).".to_owned();
                        Some(ResetStage::Custom { row, checks })
                    } else {
                        Some(ResetStage::Confirm { targets })
                    }
                }
                _ => Some(ResetStage::Custom { row, checks }),
            },
            ResetStage::Confirm { targets } => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let summary = self.apply_reset(&targets)?;
                    self.notice = summary;
                    None
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(ResetStage::Menu { row: 0 }),
                _ => Some(ResetStage::Confirm { targets }),
            },
        };
        if let Some(view) = self.settings_view.as_mut() {
            view.reset = next;
        }
        Ok(())
    }
}

pub(super) fn reset_hint(stage: &ResetStage) -> &'static str {
    match stage {
        ResetStage::Menu { .. } => "↑↓ move   Enter choose   Esc back",
        ResetStage::Custom { .. } => "↑↓ move   Space tick   Enter continue   Esc back",
        ResetStage::Confirm { .. } => "y confirm   n or Esc cancel",
    }
}

pub(super) fn draw_reset(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let Some(stage) = view.reset.as_ref() else {
        return;
    };
    let accent = crate::tui::theme::accent();
    let heading = |text: &str| {
        Line::from(Span::styled(
            text.to_owned(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let marker = |selected: bool| {
        Span::styled(
            if selected { "▸ " } else { "  " },
            Style::default().fg(accent),
        )
    };
    let label = |text: &str, selected: bool| {
        Span::styled(
            text.to_owned(),
            if selected {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        )
    };
    let mut lines = Vec::new();
    match stage {
        ResetStage::Menu { row } => {
            lines.push(heading("Reset what?"));
            lines.push(Line::from(""));
            for (index, (name, _)) in MENU.iter().enumerate() {
                lines.push(Line::from(vec![
                    marker(index == *row),
                    label(name, index == *row),
                ]));
            }
        }
        ResetStage::Custom { row, checks } => {
            lines.push(heading("Reset which of these?"));
            lines.push(Line::from(""));
            for (index, target) in ALL.iter().enumerate() {
                lines.push(Line::from(vec![
                    marker(index == *row),
                    Span::styled(
                        if checks[index] { "[x] " } else { "[ ] " },
                        Style::default().fg(if checks[index] {
                            accent
                        } else {
                            Color::DarkGray
                        }),
                    ),
                    label(target.label(), index == *row),
                ]));
            }
        }
        ResetStage::Confirm { targets } => {
            lines.push(heading("This will:"));
            lines.push(Line::from(""));
            for target in targets {
                lines.push(Line::from(Span::styled(
                    format!("  • {}", app.reset_effect(*target)),
                    Style::default().fg(Color::Gray),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "This cannot be undone. Press y to confirm.",
                Style::default()
                    .fg(Color::Rgb(235, 80, 80))
                    .add_modifier(Modifier::BOLD),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

#[cfg(test)]
mod tests {
    use super::{ResetStage, ResetTarget, reset_preferences};
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{ModelProfile, ProviderProfile, PulseMode, Settings, ThemeId};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn lived_in() -> App {
        let mut settings = Settings::default();
        settings.theme = ThemeId::Sakura;
        settings.pulse = PulseMode::Off;
        settings.stats_enabled = true;
        settings.sessions_enabled = true;
        settings.default_load_claude_md = true;
        settings.load_global_claude_md = true;
        settings.ultimate_acknowledged = true;
        settings.theme_prompt_answered = true;
        settings.motion_prompt_answered = true;
        settings.stats_prompt_answered = true;
        settings.sessions_prompt_answered = true;
        settings.instructions_prompt_answered = true;
        settings.providers = vec![ProviderProfile {
            id: "multiai".to_owned(),
            name: "MultiAI".to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: "m".to_owned(),
            models: vec![ModelProfile {
                id: "m".to_owned(),
                name: String::new(),
            }],
            ..Default::default()
        }];
        settings.active_provider_id = Some("multiai".to_owned());
        settings.default_provider_id = Some("multiai".to_owned());
        settings.model = Some("m".to_owned());
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.workspace_trusted = true;
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        crate::projects::update(&app.projects_path, |registry| {
            registry.set_trusted(&root, true);
            registry.set_claude_md(&root, true);
        })
        .unwrap();
        crate::secrets::store("multiai", "secret-key").unwrap();
        app.messages
            .push(crate::provider::ChatMessage::assistant("hi".to_owned()));
        app.transcript.push(crate::tui::state::TranscriptEntry {
            kind: crate::tui::state::TranscriptKind::User,
            text: "hello".to_owned(),
        });
        app.save_session();
        app.open_settings(Section::General);
        app
    }

    fn open_reset(app: &mut App) {
        press(app, KeyCode::Right);
        for _ in 0..10 {
            press(app, KeyCode::Down);
        }
        press(app, KeyCode::Enter);
    }

    fn choose(app: &mut App, entry: usize) {
        open_reset(app);
        for _ in 0..entry {
            press(app, KeyCode::Down);
        }
        press(app, KeyCode::Enter);
    }

    fn root() -> std::path::PathBuf {
        std::env::current_dir().unwrap().canonicalize().unwrap()
    }

    #[test]
    fn nothing_is_deleted_until_the_user_confirms() {
        let mut app = lived_in();
        choose(&mut app, 2); // Sessions
        assert!(matches!(
            app.settings_view.as_ref().and_then(|v| v.reset.clone()),
            Some(ResetStage::Confirm { .. })
        ));
        assert_eq!(crate::session::list_in(&app.session_dir, None).len(), 1);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(
            crate::session::list_in(&app.session_dir, None).len(),
            1,
            "cancelled"
        );
        choose_again(&mut app, 2);
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            crate::session::list_in(&app.session_dir, None).len(),
            1,
            "escaped"
        );
    }

    /// Picks an entry when the menu is already open.
    fn choose_again(app: &mut App, entry: usize) {
        if let Some(view) = app.settings_view.as_mut() {
            view.reset = Some(ResetStage::Menu { row: entry });
        }
        press(app, KeyCode::Enter);
    }

    #[test]
    fn resetting_sessions_deletes_only_sessions() {
        let mut app = lived_in();
        choose(&mut app, 2);
        press(&mut app, KeyCode::Char('y'));
        assert!(crate::session::list_in(&app.session_dir, None).is_empty());
        assert_eq!(app.settings.providers.len(), 1, "providers untouched");
        assert!(app.workspace_trusted);
        assert_eq!(app.settings.theme, ThemeId::Sakura);
        assert!(app.notice.contains("Reset: sessions"), "{}", app.notice);
        assert!(
            app.settings_view
                .as_ref()
                .is_some_and(|v| v.reset.is_none())
        );
    }

    #[test]
    fn resetting_trusted_folders_forgets_trust_and_asks_again() {
        let mut app = lived_in();
        choose(&mut app, 1);
        press(&mut app, KeyCode::Char('y'));
        assert!(!app.workspace_trusted && app.trust_prompt);
        assert!(!crate::projects::is_trusted_at(&app.projects_path, &root()));
        let registry = crate::projects::Registry::load_from(&app.projects_path);
        assert!(
            registry.loads_claude_md(&root(), false),
            "other project choices survive"
        );
    }

    #[test]
    fn resetting_providers_removes_them_and_their_keys() {
        let mut app = lived_in();
        choose(&mut app, 3);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.settings.providers.is_empty());
        assert_eq!(
            crate::secrets::load("multiai").unwrap(),
            None,
            "the key is gone too"
        );
        assert!(app.settings.active_provider_id.is_none() && app.settings.model.is_none());
        assert_eq!(app.settings.theme, ThemeId::Sakura, "preferences untouched");
    }

    #[test]
    fn resetting_settings_restores_preferences_but_keeps_providers_and_answers() {
        let mut app = lived_in();
        choose(&mut app, 4);
        press(&mut app, KeyCode::Char('y'));
        let settings = &app.settings;
        assert_eq!(settings.theme, ThemeId::Cool);
        assert_eq!(settings.pulse, PulseMode::Words);
        assert!(!settings.stats_enabled && !settings.sessions_enabled);
        assert!(!settings.default_load_claude_md && !settings.load_global_claude_md);
        assert!(!settings.ultimate_acknowledged);
        assert_eq!(settings.providers.len(), 1);
        assert_eq!(settings.model.as_deref(), Some("m"));
        assert!(settings.theme_prompt_answered && settings.instructions_prompt_answered);
    }

    #[test]
    fn re_running_the_wizard_needs_no_confirmation_and_deletes_nothing() {
        let mut app = lived_in();
        assert!(app.wizard.is_none());
        choose(&mut app, 5);
        assert!(app.wizard.is_some(), "the wizard starts straight away");
        assert!(!app.settings.theme_prompt_answered);
        assert_eq!(crate::session::list_in(&app.session_dir, None).len(), 1);
        assert_eq!(app.settings.providers.len(), 1);
        assert_eq!(app.settings.theme, ThemeId::Sakura);
        assert!(
            app.settings_view.is_none(),
            "settings close so the wizard is visible"
        );
    }

    #[test]
    fn everything_resets_everything_and_starts_over() {
        let mut app = lived_in();
        choose(&mut app, 0);
        press(&mut app, KeyCode::Char('y'));
        assert!(crate::session::list_in(&app.session_dir, None).is_empty());
        assert!(app.settings.providers.is_empty());
        assert_eq!(crate::secrets::load("multiai").unwrap(), None);
        assert!(!app.workspace_trusted);
        let registry = crate::projects::Registry::load_from(&app.projects_path);
        assert!(!registry.loads_claude_md(&root(), false));
        assert_eq!(app.settings.theme, ThemeId::Cool);
        assert!(app.wizard.is_some(), "setup runs again");
        assert!(!app.settings.sessions_prompt_answered);
    }

    #[test]
    fn custom_lets_you_tick_exactly_what_to_reset() {
        let mut app = lived_in();
        choose(&mut app, 6);
        press(&mut app, KeyCode::Enter);
        assert!(app.notice.contains("Tick at least one"), "{}", app.notice);
        press(&mut app, KeyCode::Char(' ')); // trusted folders
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' ')); // settings
        press(&mut app, KeyCode::Enter);
        match app.settings_view.as_ref().and_then(|v| v.reset.clone()) {
            Some(ResetStage::Confirm { targets }) => {
                assert_eq!(
                    targets,
                    [ResetTarget::TrustedFolders, ResetTarget::Settings]
                );
            }
            other => panic!("expected a confirmation, got {other:?}"),
        }
        press(&mut app, KeyCode::Char('y'));
        assert!(!app.workspace_trusted);
        assert_eq!(app.settings.theme, ThemeId::Cool);
        assert_eq!(
            crate::session::list_in(&app.session_dir, None).len(),
            1,
            "sessions kept"
        );
        assert_eq!(app.settings.providers.len(), 1, "providers kept");
    }

    #[test]
    fn the_confirmation_says_how_much_will_go() {
        let mut app = lived_in();
        choose(&mut app, 2);
        let shown = {
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30))
                .expect("terminal");
            terminal
                .draw(|frame| crate::tui::render::draw(frame, &app, 0))
                .expect("draw");
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        assert!(shown.contains("delete 1 saved session"), "{shown}");
        assert!(shown.contains("cannot be undone"), "{shown}");
    }

    #[test]
    fn reset_preferences_is_idempotent_and_keeps_the_connections() {
        let app = lived_in();
        let once = reset_preferences(&app.settings);
        let twice = reset_preferences(&once);
        assert_eq!(
            toml::to_string(&once).unwrap(),
            toml::to_string(&twice).unwrap()
        );
        assert_eq!(once.providers.len(), 1);
    }
}
