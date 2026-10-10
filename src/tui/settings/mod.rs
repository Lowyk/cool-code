mod appearance;
pub(in crate::tui) mod auto_mode;
mod auto_switch;
mod general;
mod models;
mod privacy;
mod providers;
pub(in crate::tui) use providers::remaining_bar;
mod reset;
pub(super) mod sync;
mod workflow_size;

use crate::tui::dialog::{Dialog, Routed, Tone, draw_dialog, hint_style, route, window};
use crate::tui::mouse::{Click, Hits, Row as MouseRow, line_rect};
use crate::tui::settings::appearance::draw_appearance;
use crate::tui::settings::auto_mode::draw_auto_mode;
use crate::tui::settings::auto_switch::draw_auto_switch;
use crate::tui::settings::general::draw_general;
use crate::tui::settings::models::{ModelEdit, draw_models};
use crate::tui::settings::privacy::{
    draw_privacy, draw_privacy_sub, privacy_confirm_question, privacy_sub_hint,
};
use crate::tui::settings::providers::draw_providers;
use crate::tui::settings::reset::{ResetStage, draw_reset, reset_hint};
use crate::tui::settings::workflow_size::{WorkflowChooser, chooser_hint, draw_workflow_chooser};
use crate::tui::state::App;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum Section {
    General,
    Appearance,
    Providers,
    Models,
    AutoMode,
    AutoSwitch,
    Privacy,
}

impl Section {
    pub(in crate::tui) const ALL: [Section; 7] = [
        Section::General,
        Section::Appearance,
        Section::Providers,
        Section::Models,
        Section::AutoMode,
        Section::AutoSwitch,
        Section::Privacy,
    ];

    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Providers => "Providers",
            Section::Models => "Models",
            Section::AutoMode => "Auto Mode",
            Section::AutoSwitch => "Auto-switch",
            Section::Privacy => "Privacy",
        }
    }

    fn index(self) -> usize {
        Section::ALL
            .iter()
            .position(|section| *section == self)
            .unwrap_or(0)
    }
}

const SIDEBAR_WIDTH: u16 = 18;
const COLLAPSE_BELOW_WIDTH: u16 = 70;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum Focus {
    Sidebar,
    Content,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct SettingsView {
    pub(in crate::tui) section: Section,
    pub(in crate::tui) focus: Focus,
    pub(in crate::tui) row: usize,
    pub(in crate::tui) confirm_delete: bool,
    pub(in crate::tui) model_edit: Option<ModelEdit>,
    /// The Reset menu, while it is open (it replaces the General rows).
    pub(in crate::tui) reset: Option<ResetStage>,
    /// The Dynamic workflows size chooser, while it is open (it replaces the General rows).
    pub(in crate::tui) workflow_chooser: Option<WorkflowChooser>,
    pub(in crate::tui) tree: crate::tui::widgets::tree::TreeState,
    pub(in crate::tui) privacy_sub: Option<crate::tui::settings::privacy::PrivacySub>,
}

impl SettingsView {
    pub(in crate::tui) fn open(section: Section) -> SettingsView {
        SettingsView {
            section,
            focus: Focus::Sidebar,
            row: 0,
            confirm_delete: false,
            model_edit: None,
            reset: None,
            workflow_chooser: None,
            tree: crate::tui::widgets::tree::TreeState::default(),
            privacy_sub: None,
        }
    }
}

impl App {
    pub(in crate::tui) fn open_settings(&mut self, section: Section) {
        self.settings_view = Some(SettingsView::open(section));
    }

    fn handle_section_key(&mut self, section: Section, key: event::KeyEvent) -> Result<()> {
        match section {
            Section::General => self.handle_general_key(key),
            Section::Appearance => self.handle_appearance_key(key),
            Section::Providers => self.handle_providers_key(key),
            Section::Models => self.handle_models_key(key),
            Section::AutoMode => self.handle_auto_mode_key(key),
            Section::AutoSwitch => self.handle_auto_switch_key(key),
            Section::Privacy => self.handle_privacy_key(key),
        }
    }

    pub(in crate::tui) fn handle_settings_view_key(&mut self, key: event::KeyEvent) -> Result<()> {
        // A notice describes the last thing that happened; the next key starts afresh.
        self.notice.clear();
        // Open forms receive every key so shortcut letters can be typed into fields.
        if self.provider_form.is_some() {
            return self.handle_provider_form(key);
        }
        if self.chain_form.is_some() {
            return self.handle_chain_form(key);
        }
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        if view.reset.is_some() {
            return self.handle_reset_key(key);
        }
        if view.workflow_chooser.is_some() {
            return self.handle_workflow_chooser_key(key);
        }
        if view.confirm_delete {
            // The dialog turns a key or a click into y or n, which the section then handles as it
            // always has: y deletes, anything else cancels.
            let section = view.section;
            let view = view.clone();
            let dialog = delete_dialog(self, &view);
            return match route(&dialog, &mut self.dialog_focus, key) {
                Routed::Press(code) => {
                    self.handle_section_key(section, event::KeyEvent::from(code))
                }
                Routed::Moved | Routed::Other => Ok(()),
            };
        }
        if view.model_edit.is_some() || view.privacy_sub.is_some() {
            let section = view.section;
            return self.handle_section_key(section, key);
        }
        match view.focus {
            Focus::Sidebar => match key.code {
                KeyCode::Up | KeyCode::Down => {
                    let index = view.section.index();
                    let next = if key.code == KeyCode::Up {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(Section::ALL.len() - 1)
                    };
                    view.section = Section::ALL[next];
                    view.row = 0;
                }
                KeyCode::Right | KeyCode::Enter | KeyCode::Tab => {
                    view.focus = Focus::Content;
                    if view.section == Section::Providers {
                        let selected = view.row;
                        self.refresh_key_shape(selected, false);
                        self.start_limits_fetch(selected, false);
                    }
                }
                KeyCode::Esc => self.settings_view = None,
                _ => {}
            },
            Focus::Content => match key.code {
                KeyCode::Esc | KeyCode::Left => view.focus = Focus::Sidebar,
                _ => {
                    let section = view.section;
                    self.handle_section_key(section, key)?
                }
            },
        }
        Ok(())
    }
}

pub(in crate::tui) fn draw_settings_view(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let Some(view) = app.settings_view.as_ref() else {
        return;
    };
    frame.render_widget(Clear, area);
    let block = window("Settings", Tone::Normal)
        .style(Style::default().bg(crate::tui::theme::panel_alt()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 10 {
        return;
    }
    app.hits.wheel_arrows(area);
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    let footer = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);

    let content = if inner.width >= COLLAPSE_BELOW_WIDTH {
        let sidebar = Rect::new(body.x, body.y, SIDEBAR_WIDTH, body.height);
        draw_sidebar(frame, sidebar, view, &app.hits);
        Rect::new(
            body.x + SIDEBAR_WIDTH + 2,
            body.y,
            body.width - SIDEBAR_WIDTH - 2,
            body.height,
        )
    } else {
        let switcher = Line::from(Span::styled(
            format!("‹ {} ›", view.section.label()),
            Style::default()
                .fg(crate::tui::theme::accent())
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(switcher).alignment(Alignment::Center),
            Rect::new(body.x, body.y, body.width, 1),
        );
        Rect::new(
            body.x,
            body.y + 2,
            body.width,
            body.height.saturating_sub(2),
        )
    };

    match view.section {
        Section::General if view.reset.is_some() => draw_reset(frame, content, app, view),
        Section::General if view.workflow_chooser.is_some() => {
            draw_workflow_chooser(frame, content, app, view)
        }
        Section::General => draw_general(frame, content, app, view),
        Section::Appearance => draw_appearance(frame, content, app, view),
        Section::Providers => draw_providers(frame, content, app, view),
        Section::Models => draw_models(frame, content, app, view),
        Section::AutoMode => draw_auto_mode(frame, content, app, view),
        Section::AutoSwitch => draw_auto_switch(frame, content, app, view),
        Section::Privacy => match &view.privacy_sub {
            Some(sub) => draw_privacy_sub(frame, content, app, sub),
            None => draw_privacy(frame, content, app, view),
        },
    }
    let footer_line = if !app.notice.is_empty() {
        // The settings screen covers the notice line, so what just happened is shown here.
        Span::styled(
            app.notice.clone(),
            Style::default().fg(Color::Rgb(240, 210, 90)),
        )
    } else {
        Span::styled(footer_hint(view), hint_style())
    };
    frame.render_widget(Paragraph::new(footer_line), footer);
    if view.confirm_delete {
        draw_dialog(
            frame,
            area,
            &delete_dialog(app, view),
            &app.dialog_focus,
            &app.hits,
        );
    }
}

/// A click on row `index` of a section that keeps its highlighted row in `view.row`: it moves
/// the keyboard focus to the section first if the sidebar has it.
pub(in crate::tui) fn content_row(view: &SettingsView, index: usize) -> Click {
    Click::Row(MouseRow::new(index, view.row).focus(focus_key(view)))
}

/// The key that moves the keyboard focus from the sidebar to the section, while the sidebar
/// has it.
pub(in crate::tui) fn focus_key(view: &SettingsView) -> Option<KeyCode> {
    (view.focus == Focus::Sidebar).then_some(KeyCode::Right)
}

fn draw_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, view: &SettingsView, hits: &Hits) {
    // While a sub-screen is open Esc belongs to it, so the sidebar cannot take the focus back.
    let reachable = view.reset.is_none()
        && view.workflow_chooser.is_none()
        && view.model_edit.is_none()
        && view.privacy_sub.is_none()
        && !view.confirm_delete;
    if reachable {
        for (index, _) in Section::ALL.iter().enumerate() {
            hits.click(
                line_rect(area, index),
                Click::Row(
                    MouseRow::new(index, view.section.index())
                        .activate(Some(KeyCode::Right))
                        .focus((view.focus == Focus::Content).then_some(KeyCode::Esc)),
                ),
            );
        }
    }
    let lines = Section::ALL
        .iter()
        .map(|section| {
            let selected = *section == view.section;
            let style = match (selected, view.focus) {
                (true, Focus::Sidebar) => Style::default()
                    .fg(crate::tui::theme::accent())
                    .add_modifier(Modifier::BOLD),
                (true, Focus::Content) => Style::default().fg(Color::White),
                (false, _) => Style::default().fg(Color::Gray),
            };
            Line::from(vec![
                Span::styled(
                    if selected { "▸ " } else { "  " },
                    Style::default().fg(crate::tui::theme::accent()),
                ),
                Span::styled(section.label(), style),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
    if area.height > 0 {
        let separator = (0..area.height)
            .map(|_| Line::from("│"))
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(separator).style(Style::default().fg(Color::DarkGray)),
            Rect::new(area.right(), area.y, 1, area.height),
        );
    }
}

/// The confirmation before something in Settings is deleted: its title, question and the label
/// of the button that deletes.
fn delete_dialog(app: &App, view: &SettingsView) -> Dialog<'static> {
    let (title, question, yes) = match view.section {
        Section::Models => {
            let rows = view.tree.rows(&app.settings, false);
            let id = view
                .tree
                .current(&rows)
                .and_then(|row| row.target.as_ref())
                .map(|target| target.id.clone())
                .unwrap_or_default();
            ("Remove model", format!("Remove {id}?"), "Remove")
        }
        Section::AutoSwitch => {
            let id = app
                .settings
                .model_chains
                .get(view.row)
                .map(|chain| chain.id.as_str())
                .unwrap_or("this chain");
            ("Delete chain", format!("Delete chain {id}?"), "Delete")
        }
        Section::Privacy if view.privacy_sub.is_some() => (
            "Remove redaction value",
            "Remove this redaction value?".to_owned(),
            "Remove",
        ),
        Section::Privacy => {
            let (title, question, yes) = privacy_confirm_question(view.row);
            (title, question.to_owned(), yes)
        }
        Section::General | Section::Appearance | Section::Providers | Section::AutoMode => {
            let name = app
                .settings
                .providers
                .get(view.row)
                .map(|profile| profile.name.as_str())
                .unwrap_or("this provider");
            (
                "Delete provider",
                format!("Delete {name} and its saved API key?"),
                "Delete",
            )
        }
    };
    Dialog::confirm(
        "settings-delete",
        title,
        Tone::Danger,
        vec![Line::from(question)],
        yes,
        "Cancel",
    )
}

fn footer_hint(view: &SettingsView) -> &'static str {
    if let Some(stage) = &view.reset {
        return reset_hint(stage);
    }
    if let Some(chooser) = &view.workflow_chooser {
        return chooser_hint(chooser);
    }
    if view.model_edit.is_some() {
        return "Enter confirm   Esc cancel";
    }
    if let Some(sub) = &view.privacy_sub {
        return privacy_sub_hint(sub);
    }
    match (view.focus, view.section) {
        (Focus::Sidebar, _) => "↑↓ section   →/Enter open   Esc close",
        (Focus::Content, Section::Models) => {
            "↑↓ move   n add   r rename   x remove   ← sections   Esc back"
        }
        (Focus::Content, Section::General | Section::Appearance) => {
            "↑↓ move   Enter change   ← sections   Esc back"
        }
        (Focus::Content, Section::Providers) => {
            "↑↓ move   Enter edit   n add   d default   a auto   f models   u usage   x delete   Esc back"
        }
        (Focus::Content, Section::AutoMode) => {
            "↑↓ move   Enter/Space choose or remove   u/d reorder   ← sections   Esc back"
        }
        (Focus::Content, Section::AutoSwitch) => {
            "↑↓ move   Enter edit   n new   Space activate   x delete   Esc back"
        }
        (Focus::Content, Section::Privacy) => "↑↓ move   Enter change   ← sections   Esc back",
    }
}

#[cfg(test)]
mod tests {
    use super::{Focus, Section, SettingsView};
    use crate::Settings;
    use crate::tui::render::draw;
    use crate::tui::state::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
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

    #[test]
    fn settings_opens_full_screen_with_sidebar_sections() {
        let mut app = app();
        app.input = "/settings".to_owned();
        app.submit().expect("submit");
        assert_eq!(
            app.settings_view,
            Some(SettingsView::open(Section::General))
        );
        let text = screen(&app, 100, 30);
        for label in [
            "General",
            "Appearance",
            "Providers",
            "Models",
            "Auto-switch",
            "Privacy",
        ] {
            assert!(text.contains(label), "{label} missing:\n{text}");
        }
        assert!(text.contains("Effort"), "{text}");
    }

    #[test]
    fn settings_collapses_sidebar_below_70_columns() {
        let mut app = app();
        app.open_settings(Section::General);
        let narrow = screen(&app, 60, 20);
        assert!(narrow.contains("‹ General ›"), "{narrow}");
        assert!(!narrow.contains("Auto-switch"), "{narrow}");
        let _ = screen(&app, 40, 10);
        let _ = screen(&app, 20, 5);
    }

    #[test]
    fn general_pulse_row_cycles_modes() {
        use crate::PulseMode;
        let mut app = app();
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus");
        for _ in 0..4 {
            app.handle_settings_view_key(key(KeyCode::Down))
                .expect("down");
        }
        assert_eq!(app.settings.pulse, PulseMode::Words);
        for expected in [PulseMode::Characters, PulseMode::Off, PulseMode::Words] {
            app.handle_settings_view_key(key(KeyCode::Enter))
                .expect("cycle");
            assert_eq!(app.settings.pulse, expected);
        }
    }

    #[test]
    fn general_usage_stats_row_toggles_recording() {
        let mut app = app();
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus");
        for _ in 0..5 {
            app.handle_settings_view_key(key(KeyCode::Down))
                .expect("down");
        }
        assert!(!app.settings.stats_enabled);
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("on");
        assert!(app.settings.stats_enabled && app.settings.stats_prompt_answered);
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("off");
        assert!(!app.settings.stats_enabled);
    }

    #[test]
    fn every_general_row_is_visible_on_an_ordinary_terminal() {
        let mut app = app();
        app.open_settings(Section::General);
        let shown = screen(&app, 80, 24);
        for label in [
            "Model",
            "Effort",
            "Workspace trust",
            "Usage stats",
            "Save sessions",
            "Load CLAUDE.md",
            "Load AGENTS.md",
            "Global CLAUDE.md",
            "Dynamic workflows",
            "Usage warnings",
            "Mouse",
            "Reset",
        ] {
            assert!(
                shown.contains(label),
                "{label} is cut off:
{shown}"
            );
        }
    }

    #[test]
    fn the_dynamic_workflows_row_unlocks_and_relocks_the_tiers() {
        let mut app = app();
        assert!(!app.settings.workflows_unlocked());
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus");
        for _ in 0..10 {
            app.handle_settings_view_key(key(KeyCode::Down))
                .expect("down");
        }
        for code in [KeyCode::Enter, KeyCode::Down, KeyCode::Enter] {
            app.handle_settings_view_key(key(code))
                .expect("choose Small");
        }
        assert!(app.settings.workflows_unlocked());
        assert!(app.notice.contains("unlocked"), "{}", app.notice);
        app.settings.effort = crate::Effort::Ultimate;
        for code in [KeyCode::Enter, KeyCode::Up, KeyCode::Enter] {
            app.handle_settings_view_key(key(code)).expect("choose Off");
        }
        assert!(!app.settings.workflows_unlocked());
        assert_eq!(
            app.settings.effort,
            crate::Effort::Max,
            "dropped out of Ultimate"
        );
        assert!(app.notice.contains("locked"), "{}", app.notice);
    }

    #[test]
    fn the_mouse_row_switches_mouse_capture_and_is_saved() {
        let mut app = app();
        assert!(app.settings.mouse, "on by default");
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus");
        for _ in 0..14 {
            app.handle_settings_view_key(key(KeyCode::Down))
                .expect("down");
        }
        assert!(screen(&app, 100, 30).contains("Mouse"));
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("off");
        assert!(!app.settings.mouse);
        assert!(!crate::read_settings().expect("saved").mouse);
        assert!(app.notice.contains("Shift"), "{}", app.notice);
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("on");
        assert!(app.settings.mouse);
    }

    #[test]
    fn general_save_sessions_row_is_off_by_default_and_toggles() {
        let mut app = app();
        assert!(!app.settings.sessions_enabled);
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus");
        for _ in 0..6 {
            app.handle_settings_view_key(key(KeyCode::Down))
                .expect("down");
        }
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("on");
        assert!(app.settings.sessions_enabled && app.settings.sessions_prompt_answered);
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("off");
        assert!(!app.settings.sessions_enabled);
        assert!(screen(&app, 100, 30).contains("Save sessions"));
    }

    #[test]
    fn general_rows_open_the_matching_pickers() {
        let mut app = app();
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus content");
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("model row");
        assert!(app.model_picker.take().is_some());
        app.handle_settings_view_key(key(KeyCode::Down))
            .expect("down");
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("effort row");
        assert!(app.picker);
        app.picker = false;
        app.handle_settings_view_key(key(KeyCode::Down))
            .expect("down");
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("mode row");
        assert!(app.mode_picker);
    }

    #[test]
    fn esc_from_content_returns_to_sidebar_then_closes() {
        let mut app = app();
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("right");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.focus),
            Some(Focus::Content)
        );
        app.handle_settings_view_key(key(KeyCode::Esc))
            .expect("esc");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.focus),
            Some(Focus::Sidebar)
        );
        app.handle_settings_view_key(key(KeyCode::Esc))
            .expect("esc");
        assert!(app.settings_view.is_none());
    }

    #[test]
    fn slash_provider_and_chain_open_their_sections() {
        let mut app = app();
        app.input = "/provider".to_owned();
        app.submit().expect("provider");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.section),
            Some(Section::Providers)
        );
        app.input = "/chain".to_owned();
        app.submit().expect("chain");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.section),
            Some(Section::AutoSwitch)
        );
    }

    #[test]
    fn sidebar_up_moves_to_previous_section() {
        let mut app = app();
        app.open_settings(Section::Providers);
        app.handle_settings_view_key(key(KeyCode::Up)).expect("up");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.section),
            Some(Section::Appearance)
        );
        app.handle_settings_view_key(key(KeyCode::Up)).expect("up");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.section),
            Some(Section::General)
        );
        app.handle_settings_view_key(key(KeyCode::Up))
            .expect("up at top");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.section),
            Some(Section::General)
        );
    }
}

#[cfg(test)]
mod tiny_terminal_tests {
    use super::Section;
    use crate::tui::render::draw;
    use crate::tui::state::App;
    use crate::{ModelProfile, ProviderProfile, Settings};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn every_section_draws_on_tiny_terminals_without_panicking() {
        let mut settings = Settings::default();
        settings.providers = vec![ProviderProfile {
            id: "groq".to_owned(),
            name: "groq".to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: "qwen".to_owned(),
            models: vec![ModelProfile {
                id: "qwen".to_owned(),
                name: String::new(),
            }],
            draft: false,
            auto_switch: true,
            base_url: None,
            ..Default::default()
        }];
        for section in Section::ALL {
            for (width, height) in [(20, 5), (12, 4), (40, 8), (69, 12)] {
                let mut app = App::new(settings.clone());
                app.trust_prompt = false;
                app.open_settings(section);
                let mut terminal =
                    Terminal::new(TestBackend::new(width, height)).expect("terminal");
                terminal
                    .draw(|frame| draw(frame, &app, 0))
                    .unwrap_or_else(|_| panic!("{section:?} at {width}x{height}"));
            }
        }
    }
}
