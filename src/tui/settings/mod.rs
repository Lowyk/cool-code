mod auto_switch;
mod general;
mod models;
mod privacy;
mod providers;

use crate::tui::settings::auto_switch::draw_auto_switch;
use crate::tui::settings::general::draw_general;
use crate::tui::settings::models::{ModelEdit, draw_models, model_rows};
use crate::tui::settings::privacy::{draw_privacy, privacy_confirm_question};
use crate::tui::settings::providers::draw_providers;
use crate::tui::state::App;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum Section {
    General,
    Providers,
    Models,
    AutoSwitch,
    Privacy,
}

impl Section {
    pub(in crate::tui) const ALL: [Section; 5] = [
        Section::General,
        Section::Providers,
        Section::Models,
        Section::AutoSwitch,
        Section::Privacy,
    ];

    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Providers => "Providers",
            Section::Models => "Models",
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
const ACCENT: Color = Color::Rgb(98, 213, 244);

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
}

impl SettingsView {
    pub(in crate::tui) fn open(section: Section) -> SettingsView {
        SettingsView {
            section,
            focus: Focus::Sidebar,
            row: 0,
            confirm_delete: false,
            model_edit: None,
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
            Section::Providers => self.handle_providers_key(key),
            Section::Models => self.handle_models_key(key),
            Section::AutoSwitch => self.handle_auto_switch_key(key),
            Section::Privacy => self.handle_privacy_key(key),
        }
    }

    pub(in crate::tui) fn handle_settings_view_key(&mut self, key: event::KeyEvent) -> Result<()> {
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
        if view.confirm_delete || view.model_edit.is_some() {
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
                KeyCode::Right | KeyCode::Enter | KeyCode::Tab => view.focus = Focus::Content,
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
    let block = Block::default()
        .title(" Settings ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(Color::Rgb(22, 24, 27)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 10 {
        return;
    }
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    let footer = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);

    let content = if inner.width >= COLLAPSE_BELOW_WIDTH {
        let sidebar = Rect::new(body.x, body.y, SIDEBAR_WIDTH, body.height);
        draw_sidebar(frame, sidebar, view);
        Rect::new(
            body.x + SIDEBAR_WIDTH + 2,
            body.y,
            body.width - SIDEBAR_WIDTH - 2,
            body.height,
        )
    } else {
        let switcher = Line::from(Span::styled(
            format!("‹ {} ›", view.section.label()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
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
        Section::General => draw_general(frame, content, app, view),
        Section::Providers => draw_providers(frame, content, app, view),
        Section::Models => draw_models(frame, content, app, view),
        Section::AutoSwitch => draw_auto_switch(frame, content, app, view),
        Section::Privacy => draw_privacy(frame, content, app, view),
    }
    let footer_line = if view.confirm_delete {
        let question = confirm_question(app, view);
        Span::styled(
            question,
            Style::default()
                .fg(Color::Rgb(235, 80, 80))
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(footer_hint(view), Style::default().fg(Color::DarkGray))
    };
    frame.render_widget(Paragraph::new(footer_line), footer);
}

fn draw_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, view: &SettingsView) {
    let lines = Section::ALL
        .iter()
        .map(|section| {
            let selected = *section == view.section;
            let style = match (selected, view.focus) {
                (true, Focus::Sidebar) => Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                (true, Focus::Content) => Style::default().fg(Color::White),
                (false, _) => Style::default().fg(Color::Gray),
            };
            Line::from(vec![
                Span::styled(
                    if selected { "▸ " } else { "  " },
                    Style::default().fg(ACCENT),
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

fn confirm_question(app: &App, view: &SettingsView) -> String {
    match view.section {
        Section::Models => {
            let rows = model_rows(&app.settings);
            let id = rows
                .get(view.row.min(rows.len().saturating_sub(1)))
                .map(|(_, (provider, model))| {
                    app.settings.providers[*provider].models[*model].id.clone()
                })
                .unwrap_or_default();
            format!("Remove {id}? y/n")
        }
        Section::AutoSwitch => {
            let id = app
                .settings
                .model_chains
                .get(view.row)
                .map(|chain| chain.id.as_str())
                .unwrap_or("this chain");
            format!("Delete chain {id}? y/n")
        }
        Section::Privacy => privacy_confirm_question(view.row).to_owned(),
        Section::General | Section::Providers => {
            let name = app
                .settings
                .providers
                .get(view.row)
                .map(|profile| profile.name.as_str())
                .unwrap_or("this provider");
            format!("Delete {name} and its saved API key? y/n")
        }
    }
}

fn footer_hint(view: &SettingsView) -> &'static str {
    if view.model_edit.is_some() {
        return "Enter confirm   Esc cancel";
    }
    match (view.focus, view.section) {
        (Focus::Sidebar, _) => "↑↓ section   →/Enter open   Esc close",
        (Focus::Content, Section::Models) => {
            "↑↓ move   n add   r rename   x remove   ← sections   Esc back"
        }
        (Focus::Content, Section::General) => "↑↓ move   Enter change   ← sections   Esc back",
        (Focus::Content, Section::Providers) => {
            "↑↓ move   Enter edit   n add   d default   a auto   x delete   Esc back"
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
        for label in ["General", "Providers", "Models", "Auto-switch", "Privacy"] {
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
