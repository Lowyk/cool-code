mod general;

use crate::tui::render::settings::draw_settings_content;
use crate::tui::settings::general::draw_general;
use crate::tui::state::{App, SettingsTab};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

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

    // Unported sections still render and handle keys through the legacy tab code.
    fn legacy_tab(self) -> SettingsTab {
        match self {
            Section::General | Section::Models => SettingsTab::General,
            Section::Providers => SettingsTab::Providers,
            Section::AutoSwitch => SettingsTab::AutoSwitch,
            Section::Privacy => SettingsTab::Privacy,
        }
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
}

impl SettingsView {
    pub(in crate::tui) fn open(section: Section) -> SettingsView {
        SettingsView {
            section,
            focus: Focus::Sidebar,
            row: 0,
        }
    }
}

impl App {
    pub(in crate::tui) fn open_settings(&mut self, section: Section) {
        self.settings_view = Some(SettingsView::open(section));
        self.settings_tab = section.legacy_tab();
    }

    pub(in crate::tui) fn handle_settings_view_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
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
                    self.settings_tab = view.section.legacy_tab();
                }
                KeyCode::Right | KeyCode::Enter | KeyCode::Tab => view.focus = Focus::Content,
                KeyCode::Esc => self.settings_view = None,
                _ => {}
            },
            Focus::Content => match key.code {
                KeyCode::Esc | KeyCode::Left => view.focus = Focus::Sidebar,
                _ => match view.section {
                    Section::General => self.handle_general_key(key)?,
                    Section::Models => {}
                    _ => self.handle_settings_key(key)?,
                },
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
        Section::Models => frame.render_widget(
            Paragraph::new("Model management is coming soon. Use Providers to edit models.")
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
            content,
        ),
        _ => draw_settings_content(frame, content, app),
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            footer_hint(view),
            Style::default().fg(Color::DarkGray),
        )),
        footer,
    );
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

fn footer_hint(view: &SettingsView) -> &'static str {
    match (view.focus, view.section) {
        (Focus::Sidebar, _) => "↑↓ section   →/Enter open   Esc close",
        (Focus::Content, Section::General) => "↑↓ move   Enter change   ← sections   Esc back",
        (Focus::Content, _) => "← sections   Esc back",
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
