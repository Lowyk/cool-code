//! Settings > Auto Mode: choosing the guard models that decide, one action at a time, whether
//! Auto mode may go ahead without asking the user.

use crate::guard::{AutoGuard, GuardKind, guard_kind, recommended_family};
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::tui::widgets::list::{ListItem, ListState, draw_list};
use crate::{Settings, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

/// What the user is told when they pick Auto before it is set up.
pub(in crate::tui) const ACTIVATE_NOTICE: &str = "Go to Settings > Auto Mode to activate";

/// What the user is told when Auto mode had to be turned off.
pub(in crate::tui) const AUTO_OFF_NOTICE: &str = "Auto mode needs a judge model, so the mode is now Manual. Go to Settings > Auto Mode to set one up.";

const YOURS: &str = "Your guards (asked in this order)";
const RECOMMENDED: &str = "Recommended";
const OTHER: &str = "Other models";

/// The recommended families, in the order they are listed.
const FAMILIES: [&str; 6] = [
    "Luna",
    "Haiku",
    "Flash",
    "Flash-Lite",
    "Safety",
    "Prompt Guard",
];

/// One selectable model in the list.
pub(super) struct GuardRow {
    pub(super) guard: AutoGuard,
    pub(super) item: ListItem,
}

fn provider_name<'a>(settings: &'a Settings, id: &str) -> &'a str {
    settings
        .providers
        .iter()
        .find(|profile| profile.id == id)
        .map_or("unknown provider", |profile| profile.name.as_str())
}

/// Every model of every finished provider, as (provider name, guard).
fn candidates(settings: &Settings) -> Vec<(String, AutoGuard)> {
    let mut found = Vec::new();
    for profile in settings.providers.iter().filter(|profile| !profile.draft) {
        let mut ids: Vec<&str> = profile
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        if !profile.model.is_empty() && !ids.contains(&profile.model.as_str()) {
            ids.push(profile.model.as_str());
        }
        for id in ids {
            found.push((
                profile.name.clone(),
                AutoGuard {
                    provider_id: profile.id.clone(),
                    model_id: id.to_owned(),
                },
            ));
        }
    }
    found
}

fn kind_note(model_id: &str) -> &'static str {
    match guard_kind(model_id) {
        GuardKind::Judge => "judge",
        GuardKind::Scanner => "injection scanner, never approves on its own",
    }
}

/// The list: the chosen guards in order, then the recommended models, then everything else.
pub(super) fn rows(settings: &Settings) -> Vec<GuardRow> {
    let mut rows = Vec::new();
    for guard in &settings.auto_guards {
        let usable = settings.settings_for_guard(guard).is_some();
        rows.push(GuardRow {
            guard: guard.clone(),
            item: ListItem {
                label: guard.model_id.clone(),
                detail: format!(
                    "{} · {}{}",
                    provider_name(settings, &guard.provider_id),
                    kind_note(&guard.model_id),
                    if usable {
                        ""
                    } else {
                        " · provider unavailable"
                    }
                ),
                group: Some(YOURS.to_owned()),
                dimmed: !usable,
                selectable: true,
                marked: true,
            },
        });
    }
    let mut recommended = Vec::new();
    let mut other = Vec::new();
    for (provider, guard) in candidates(settings) {
        let marked = settings.auto_guards.contains(&guard);
        let family = recommended_family(&guard.model_id);
        let item = ListItem {
            label: guard.model_id.clone(),
            detail: match family {
                Some("Prompt Guard") => format!("{provider} · {}", kind_note(&guard.model_id)),
                Some(family) => format!("{provider} · {family}"),
                None => provider,
            },
            group: Some(if family.is_some() { RECOMMENDED } else { OTHER }.to_owned()),
            dimmed: false,
            selectable: true,
            marked,
        };
        match family {
            Some(family) => {
                let rank = FAMILIES
                    .iter()
                    .position(|name| *name == family)
                    .unwrap_or(FAMILIES.len());
                recommended.push((rank, GuardRow { guard, item }));
            }
            None => other.push(GuardRow { guard, item }),
        }
    }
    recommended.sort_by_key(|(rank, _)| *rank);
    rows.extend(recommended.into_iter().map(|(_, row)| row));
    rows.extend(other);
    rows
}

pub(super) fn draw_auto_mode(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let ready = app.settings.auto_ready();
    let status = if ready {
        Line::styled(
            "Status: ready. Auto can now be picked in the mode selector.",
            Style::default().fg(Color::Rgb(110, 220, 130)),
        )
    } else {
        Line::styled(
            "Status: not active. Choose at least one judge model below.",
            Style::default().fg(Color::Rgb(240, 210, 90)),
        )
    };
    let header = vec![
        status,
        Line::styled(
            "Every command, edit and new file is shown to these models. They answer yes or no, and a no asks you. They are tried in order, so a later one covers for an earlier one that is out of usage.",
            Style::default().fg(Color::Gray),
        ),
        Line::styled(
            "Prompt Guard models only scan for injected instructions and never approve anything. What they check is sent to the provider you pick.",
            Style::default().fg(Color::Gray),
        ),
    ];
    let header_height = if area.height >= 18 {
        8
    } else if area.height >= 8 {
        1
    } else {
        0
    };
    if header_height > 0 {
        frame.render_widget(
            Paragraph::new(header).wrap(Wrap { trim: true }),
            Rect::new(area.x, area.y, area.width, header_height),
        );
    }
    let list_area = Rect::new(
        area.x,
        area.y + header_height,
        area.width,
        area.height.saturating_sub(header_height),
    );
    let rows = rows(&app.settings);
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "Add a provider in Settings > Providers first; its models will be listed here.",
            )
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true }),
            list_area,
        );
        return;
    }
    let items: Vec<ListItem> = rows.into_iter().map(|row| row.item).collect();
    let state = ListState {
        selected: view.row,
        filter: String::new(),
    };
    draw_list(
        frame,
        list_area,
        &items,
        &state,
        view.focus == Focus::Content,
    );
}

/// What a key in this section asks for.
enum Act {
    Toggle(AutoGuard),
    Swap(usize, usize),
}

impl App {
    pub(super) fn handle_auto_mode_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let rows = rows(&self.settings);
        let chosen = self.settings.auto_guards.len();
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        if rows.is_empty() {
            return Ok(());
        }
        let row = view.row.min(rows.len() - 1);
        let act = match key.code {
            KeyCode::Up => {
                view.row = row.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                view.row = (row + 1).min(rows.len() - 1);
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') => Some(Act::Toggle(rows[row].guard.clone())),
            KeyCode::Char('u') if row > 0 && row < chosen => {
                view.row = row - 1;
                Some(Act::Swap(row, row - 1))
            }
            KeyCode::Char('d') if row + 1 < chosen => {
                view.row = row + 1;
                Some(Act::Swap(row, row + 1))
            }
            _ => None,
        };
        match act {
            Some(Act::Toggle(guard)) => {
                let from_top = row < chosen;
                self.toggle_guard(guard.clone())?;
                // The first group grew or shrank above the cursor; keep it on the same model.
                let after = self::rows(&self.settings);
                let chosen_now = self.settings.auto_guards.len();
                if let Some(view) = self.settings_view.as_mut() {
                    view.row = if from_top {
                        row.min(after.len().saturating_sub(1))
                    } else {
                        after
                            .iter()
                            .enumerate()
                            .skip(chosen_now)
                            .find(|(_, candidate)| candidate.guard == guard)
                            .map_or(row, |(index, _)| index)
                    };
                }
                Ok(())
            }
            Some(Act::Swap(a, b)) => {
                self.settings.auto_guards.swap(a, b);
                write_settings(&self.settings)
            }
            None => Ok(()),
        }
    }

    fn toggle_guard(&mut self, guard: AutoGuard) -> Result<()> {
        let was_ready = self.settings.auto_ready();
        match self
            .settings
            .auto_guards
            .iter()
            .position(|chosen| *chosen == guard)
        {
            Some(position) => {
                self.settings.auto_guards.remove(position);
            }
            None => self.settings.auto_guards.push(guard),
        }
        write_settings(&self.settings)?;
        if self.settings.auto_ready() {
            if !was_ready {
                self.notice = "Auto mode is ready: pick it in the mode selector.".to_owned();
            }
            return Ok(());
        }
        if !self.leave_auto_if_unusable()? && was_ready {
            self.notice = "Auto mode is not available now: no judge model is chosen.".to_owned();
        }
        Ok(())
    }

    /// Auto mode without a usable judge model cannot run, so the mode drops to Manual. Returns
    /// whether the mode changed.
    pub(in crate::tui) fn leave_auto_if_unusable(&mut self) -> Result<bool> {
        if !self.settings.fall_back_from_unusable_auto() {
            return Ok(false);
        }
        self.mode_index = crate::policy::mode_index(&self.settings.permission_mode);
        write_settings(&self.settings)?;
        self.notice = AUTO_OFF_NOTICE.to_owned();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTIVATE_NOTICE, rows};
    use crate::guard::AutoGuard;
    use crate::tui::render::draw;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{ModelProfile, ProviderProfile, Settings, read_settings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn provider(id: &str, name: &str, draft: bool, models: &[&str]) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: name.to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: models.first().copied().unwrap_or_default().to_owned(),
            models: models
                .iter()
                .map(|id| ModelProfile {
                    id: (*id).to_owned(),
                    name: String::new(),
                })
                .collect(),
            draft,
            base_url: Some("https://example.invalid/v1".to_owned()),
            ..Default::default()
        }
    }

    fn settings() -> Settings {
        let mut settings = Settings::default();
        settings.providers = vec![
            provider(
                "alpha",
                "Alpha",
                false,
                &[
                    "claude-opus-5-5",
                    "claude-haiku-5-5",
                    "deepseek-v4-flash",
                    "meta-llama/llama-prompt-guard-2-22m",
                ],
            ),
            provider("half", "Half", true, &["gpt-6-luna"]),
            provider("beta", "Beta", false, &["gpt-6", "gpt-6-luna"]),
        ];
        settings
    }

    fn guard(provider: &str, model: &str) -> AutoGuard {
        AutoGuard {
            provider_id: provider.to_owned(),
            model_id: model.to_owned(),
        }
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn open(settings: Settings) -> App {
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.open_settings(Section::AutoMode);
        press(&mut app, KeyCode::Right);
        app
    }

    fn screen(app: &App) -> String {
        let (width, height) = (110, 32);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn labels(settings: &Settings, group: &str) -> Vec<String> {
        rows(settings)
            .into_iter()
            .filter(|row| {
                row.item
                    .group
                    .as_deref()
                    .is_some_and(|g| g.starts_with(group))
            })
            .map(|row| format!("{}/{}", row.guard.provider_id, row.guard.model_id))
            .collect()
    }

    #[test]
    fn models_of_finished_providers_are_sorted_into_recommended_and_the_rest() {
        let settings = settings();
        assert_eq!(
            labels(&settings, "Recommended"),
            [
                "beta/gpt-6-luna",
                "alpha/claude-haiku-5-5",
                "alpha/deepseek-v4-flash",
                "alpha/meta-llama/llama-prompt-guard-2-22m",
            ],
            "family order: Luna, Haiku, Flash, then Prompt Guard; the unfinished provider is left out"
        );
        assert_eq!(
            labels(&settings, "Other"),
            ["alpha/claude-opus-5-5", "beta/gpt-6"]
        );
        assert!(labels(&settings, "Your guards").is_empty());
    }

    #[test]
    fn chosen_guards_are_listed_first_in_order_and_marked_everywhere() {
        let mut settings = settings();
        settings.auto_guards = vec![
            guard("alpha", "deepseek-v4-flash"),
            guard("beta", "gpt-6-luna"),
        ];
        assert_eq!(
            labels(&settings, "Your guards"),
            ["alpha/deepseek-v4-flash", "beta/gpt-6-luna"]
        );
        let marked: Vec<_> = rows(&settings)
            .into_iter()
            .filter(|row| {
                row.item.marked
                    && row
                        .item
                        .group
                        .as_deref()
                        .is_some_and(|g| g.starts_with("Recommended"))
            })
            .map(|row| row.guard.model_id)
            .collect();
        assert_eq!(marked, ["gpt-6-luna", "deepseek-v4-flash"]);
    }

    #[test]
    fn enter_chooses_a_model_and_enter_again_removes_it_and_both_are_saved() {
        let mut app = open(settings());
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.auto_guards, [guard("beta", "gpt-6-luna")]);
        assert_eq!(
            read_settings().unwrap().auto_guards,
            [guard("beta", "gpt-6-luna")],
            "saved to the settings file"
        );
        // The cursor stayed on the model just chosen, so the next row down is the next model.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.settings.auto_guards,
            [
                guard("beta", "gpt-6-luna"),
                guard("alpha", "claude-haiku-5-5")
            ]
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.settings.auto_guards,
            [guard("beta", "gpt-6-luna")],
            "Enter on the same row again removes it"
        );
    }

    #[test]
    fn choosing_the_first_judge_makes_auto_ready_and_removing_the_last_leaves_it() {
        let mut settings = settings();
        settings.permission_mode = "manual".to_owned();
        let mut app = open(settings);
        assert!(!app.settings.auto_ready());
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.auto_ready());
        assert!(app.notice.contains("Auto mode is ready"), "{}", app.notice);
        app.settings.permission_mode = "auto".to_owned();
        // Removing the only judge from the first group switches Auto off.
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.auto_guards.is_empty());
        assert_eq!(app.settings.permission_mode, "manual");
        assert!(app.notice.contains("Manual"), "{}", app.notice);
    }

    #[test]
    fn a_scanner_alone_does_not_make_auto_ready() {
        let mut app = open(settings());
        for _ in 0..3 {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.settings.auto_guards,
            [guard("alpha", "meta-llama/llama-prompt-guard-2-22m")]
        );
        assert!(!app.settings.auto_ready());
    }

    #[test]
    fn guards_can_be_moved_up_and_down_in_the_first_group() {
        let mut settings = settings();
        settings.auto_guards = vec![guard("alpha", "claude-haiku-5-5"), guard("beta", "gpt-6")];
        let mut app = open(settings);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.settings.auto_guards[0], guard("beta", "gpt-6"));
        assert_eq!(
            app.settings.auto_guards[1],
            guard("alpha", "claude-haiku-5-5")
        );
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(
            app.settings.auto_guards[0],
            guard("alpha", "claude-haiku-5-5"),
            "the cursor followed the moved guard"
        );
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(
            app.settings.auto_guards[0],
            guard("alpha", "claude-haiku-5-5"),
            "already first"
        );
    }

    #[test]
    fn the_screen_explains_the_setup_and_shows_the_groups() {
        let app = open(settings());
        let text = screen(&app);
        for expected in [
            "Auto Mode",
            "not active",
            "Recommended",
            "Other models",
            "gpt-6-luna",
            "Prompt Guard",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        let mut chosen = settings();
        chosen.auto_guards = vec![guard("alpha", "claude-haiku-5-5")];
        let text = screen(&open(chosen));
        assert!(text.contains("Your guards"), "{text}");
        assert!(text.contains("ready"), "{text}");
    }

    #[test]
    fn without_a_finished_provider_the_screen_says_where_to_start() {
        let app = open(Settings::default());
        let text = screen(&app);
        assert!(text.contains("Add a provider"), "{text}");
        // Keys do nothing, and do not crash, with nothing to choose from.
        let mut app = app;
        for code in [
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char('u'),
            KeyCode::Char(' '),
        ] {
            press(&mut app, code);
        }
        assert!(app.settings.auto_guards.is_empty());
    }

    #[test]
    fn auto_is_refused_with_a_pointer_to_settings_until_a_judge_is_chosen() {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.mode_picker = true;
        app.apply_mode("auto").unwrap();
        assert_eq!(app.settings.permission_mode, "plan");
        assert_eq!(app.notice, ACTIVATE_NOTICE);
        assert_eq!(ACTIVATE_NOTICE, "Go to Settings > Auto Mode to activate");
        assert!(app.mode_picker, "the picker stays open");
        app.settings.auto_guards = vec![guard("alpha", "claude-haiku-5-5")];
        app.apply_mode("auto").unwrap();
        assert_eq!(app.settings.permission_mode, "auto");
        assert!(!app.mode_picker);
    }

    #[test]
    fn manual_can_always_be_chosen() {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.apply_mode("manual").unwrap();
        assert_eq!(app.settings.permission_mode, "manual");
    }

    #[test]
    fn the_mode_command_refuses_auto_the_same_way() {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.input = "/mode auto".to_owned();
        app.submit().unwrap();
        assert_eq!(app.settings.permission_mode, "plan");
        assert!(app.notice.contains(ACTIVATE_NOTICE), "{}", app.notice);
        app.input = "/mode manual".to_owned();
        app.submit().unwrap();
        assert_eq!(app.settings.permission_mode, "manual");
    }

    #[test]
    fn a_saved_auto_mode_without_a_judge_starts_as_manual() {
        let mut saved = settings();
        saved.permission_mode = "auto".to_owned();
        let app = App::new(saved.clone());
        assert_eq!(app.settings.permission_mode, "manual");
        assert!(app.notice.contains("Auto mode"), "{}", app.notice);
        saved.auto_guards = vec![guard("alpha", "claude-haiku-5-5")];
        let app = App::new(saved);
        assert_eq!(app.settings.permission_mode, "auto");
    }

    #[test]
    fn the_mode_picker_greys_out_auto_until_it_can_be_used() {
        let find_auto = |app: &App| {
            let mut terminal = Terminal::new(TestBackend::new(110, 32)).expect("terminal");
            terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
            let buffer = terminal.backend().buffer().clone();
            buffer
                .content()
                .windows(4)
                .find(|cells| cells.iter().map(|c| c.symbol()).collect::<String>() == "Auto")
                .map(|cells| cells[0].fg)
        };
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.mode_picker = true;
        assert_eq!(find_auto(&app), Some(Color::DarkGray));
        app.settings.auto_guards = vec![guard("alpha", "claude-haiku-5-5")];
        assert_eq!(find_auto(&app), Some(Color::Rgb(110, 220, 130)));
    }

    #[test]
    fn deleting_the_provider_behind_the_only_judge_switches_auto_off() {
        let mut saved = settings();
        saved.auto_guards = vec![guard("alpha", "claude-haiku-5-5")];
        saved.permission_mode = "auto".to_owned();
        let mut app = App::new(saved);
        assert_eq!(app.settings.permission_mode, "auto");
        app.settings
            .providers
            .retain(|profile| profile.id != "alpha");
        assert!(app.leave_auto_if_unusable().unwrap());
        assert_eq!(app.settings.permission_mode, "manual");
        assert!(app.notice.contains("Manual"), "{}", app.notice);
    }
}
