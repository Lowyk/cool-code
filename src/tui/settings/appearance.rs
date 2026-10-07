//! Settings > Appearance: the theme and the animated backdrop.

use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::tui::theme::{self, THEMES};
use crate::write_settings;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// One row per theme, then the three backdrop switches.
pub(super) const ROWS: usize = THEMES.len() + 3;
const WELCOME_BACKDROP: usize = THEMES.len();
const CHAT_BACKDROP: usize = THEMES.len() + 1;
const DIM_IN_CHAT: usize = THEMES.len() + 2;

fn switch(on: bool) -> Span<'static> {
    if on {
        Span::styled("on", Style::default().fg(Color::Rgb(110, 220, 130)))
    } else {
        Span::styled("off", Style::default().fg(Color::Gray))
    }
}

pub(super) fn draw_appearance(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let focused = view.focus == Focus::Content;
    let marker = |index: usize| {
        Span::styled(
            if focused && index == view.row {
                "▸ "
            } else {
                "  "
            },
            Style::default().fg(theme::accent()),
        )
    };
    let label_style = |index: usize| {
        if focused && index == view.row {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        }
    };
    let heading = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let mut lines = vec![heading("Theme")];
    for (index, entry) in THEMES.iter().enumerate() {
        let chosen = entry.id == app.settings.theme;
        let mut spans = vec![
            marker(index),
            Span::styled(
                if chosen { "● " } else { "  " },
                Style::default().fg(Color::Rgb(110, 220, 130)),
            ),
            Span::styled(format!("{:<15}", entry.name), label_style(index)),
            Span::styled("■", Style::default().fg(entry.accent)),
        ];
        if focused && index == view.row {
            spans.push(Span::styled(
                format!("  {}", entry.summary),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(heading("Backdrop"));
    for (index, label, value) in [
        (
            WELCOME_BACKDROP,
            "Welcome screen",
            app.settings.background_animation,
        ),
        (
            CHAT_BACKDROP,
            "While chatting",
            app.settings.backdrop_in_chat,
        ),
        (
            DIM_IN_CHAT,
            "Dim while chatting",
            app.settings.dim_backdrop_in_chat,
        ),
    ] {
        lines.push(Line::from(vec![
            marker(index),
            Span::raw("  "),
            Span::styled(format!("{label:<19}"), label_style(index)),
            switch(value),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

impl App {
    pub(super) fn handle_appearance_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Up => view.row = view.row.saturating_sub(1),
            KeyCode::Down => view.row = (view.row + 1).min(ROWS - 1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                match view.row {
                    row if row < THEMES.len() => self.settings.theme = THEMES[row].id,
                    WELCOME_BACKDROP => {
                        self.settings.background_animation = !self.settings.background_animation;
                    }
                    CHAT_BACKDROP => {
                        self.settings.backdrop_in_chat = !self.settings.backdrop_in_chat;
                    }
                    _ => {
                        self.settings.dim_backdrop_in_chat = !self.settings.dim_backdrop_in_chat;
                    }
                }
                write_settings(&self.settings)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{Settings, ThemeId};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.open_settings(Section::Appearance);
        press(&mut app, KeyCode::Right);
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn down(app: &mut App, times: usize) {
        for _ in 0..times {
            press(app, KeyCode::Down);
        }
    }

    #[test]
    fn enter_on_a_theme_row_applies_and_saves_that_theme() {
        let mut app = app();
        assert_eq!(app.settings.theme, ThemeId::Cool);
        down(&mut app, 1);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.theme, ThemeId::Galaxy);
        down(&mut app, 1);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.settings.theme, ThemeId::GalaxyVoid);
        assert!(crate::tui::theme::THEMES.len() >= 8);
    }

    #[test]
    fn the_backdrop_switches_toggle_independently() {
        let mut app = app();
        let themes = crate::tui::theme::THEMES.len();
        down(&mut app, themes);
        let (welcome, chat, dim) = (
            app.settings.background_animation,
            app.settings.backdrop_in_chat,
            app.settings.dim_backdrop_in_chat,
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.background_animation, !welcome);
        assert_eq!(app.settings.backdrop_in_chat, chat);
        down(&mut app, 1);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.backdrop_in_chat, !chat);
        down(&mut app, 1);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.dim_backdrop_in_chat, !dim);
        assert_eq!(app.settings.background_animation, !welcome);
    }

    #[test]
    fn the_cursor_stays_inside_the_section() {
        let mut app = app();
        down(&mut app, 50);
        assert_eq!(
            app.settings_view.as_ref().map(|view| view.row),
            Some(super::ROWS - 1)
        );
        for _ in 0..50 {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.settings_view.as_ref().map(|view| view.row), Some(0));
    }

    #[test]
    fn defaults_keep_the_original_look_and_a_calm_chat() {
        let settings = Settings::default();
        assert_eq!(settings.theme, ThemeId::Cool);
        assert!(settings.background_animation);
        assert!(!settings.backdrop_in_chat, "chats stay plain unless asked");
        assert!(settings.dim_backdrop_in_chat);
    }
}
