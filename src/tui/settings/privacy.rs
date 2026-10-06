use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::{provider, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub(super) const ROWS: usize = 3;

pub(super) fn draw_privacy(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let accent = Color::Rgb(98, 213, 244);
    let trust = if app.workspace_trusted {
        Span::styled("trusted", Style::default().fg(Color::Rgb(110, 220, 130)))
    } else {
        Span::styled("not trusted", Style::default().fg(Color::Rgb(255, 197, 92)))
    };
    let rows: [(&str, Span<'static>, &str); ROWS] = [
        (
            "Workspace trust",
            trust,
            "Allows COOL.md and @path reads and permission-gated edits in this folder.",
        ),
        (
            "Redaction values",
            Span::styled("clear", Style::default().fg(Color::White)),
            "Custom values live in the OS credential store. Add more with /privacy add <value>.",
        ),
        (
            "Acknowledgements",
            Span::styled(
                format!(
                    "{} provider · {} image grant",
                    app.settings.privacy_acknowledged.len(),
                    app.settings.privacy_image_acknowledged.len()
                ),
                Style::default().fg(Color::White),
            ),
            "Revoking makes flagged providers ask for consent again.",
        ),
    ];
    let focused = view.focus == Focus::Content;
    let mut lines = Vec::new();
    for (index, (label, value, help)) in rows.into_iter().enumerate() {
        let selected = focused && index == view.row;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "▸ " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(
                format!("{label:<18}"),
                if selected {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            value,
        ]));
        lines.push(Line::from(Span::styled(
            format!("    {help}"),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "Text redaction is best-effort. Images are sent unredacted only with explicit consent.",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

impl App {
    pub(super) fn handle_privacy_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        if view.confirm_delete {
            view.confirm_delete = false;
            if key.code != KeyCode::Char('y') {
                self.notice = "Nothing changed.".to_owned();
                return Ok(());
            }
            if view.row == 1 {
                provider::save_redaction_values(&[])?;
                self.notice = "Custom redaction values cleared.".to_owned();
            } else {
                self.settings.privacy_acknowledged.clear();
                self.settings.privacy_image_acknowledged.clear();
                write_settings(&self.settings)?;
                self.notice = "Privacy acknowledgements and image grants revoked.".to_owned();
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Up => view.row = view.row.saturating_sub(1),
            KeyCode::Down => view.row = (view.row + 1).min(ROWS - 1),
            KeyCode::Enter | KeyCode::Char('t') if view.row == 0 => {
                self.set_workspace_trusted(!self.workspace_trusted)?
            }
            KeyCode::Enter | KeyCode::Char('c') if view.row == 1 => view.confirm_delete = true,
            KeyCode::Enter | KeyCode::Char('r') if view.row == 2 => view.confirm_delete = true,
            _ => {}
        }
        Ok(())
    }
}

pub(super) fn privacy_confirm_question(row: usize) -> &'static str {
    if row == 1 {
        "Clear all custom redaction values? y/n"
    } else {
        "Revoke all privacy acknowledgements and image grants? y/n"
    }
}

#[cfg(test)]
mod tests {
    use crate::Settings;
    use crate::tui::render::draw;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn app() -> App {
        let mut settings = Settings::default();
        settings.privacy_acknowledged = vec!["google".to_owned()];
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.open_settings(Section::Privacy);
        press(&mut app, KeyCode::Right);
        app
    }

    #[test]
    fn privacy_revoke_requires_confirmation() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.settings.privacy_acknowledged.len(), 1);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.settings.privacy_acknowledged.is_empty());
    }

    #[test]
    fn privacy_section_shows_trust_and_acknowledgement_rows() {
        let app = app();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Workspace trust"), "{text}");
        assert!(text.contains("Redaction values"), "{text}");
        assert!(text.contains("Acknowledgements"), "{text}");
    }
}
