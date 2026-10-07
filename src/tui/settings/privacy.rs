use crate::provider::{self, PRIVACY_FAMILIES};
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::write_settings;
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
    let accent = crate::tui::theme::accent();
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
            Span::styled(
                "kept in OS credential store · c to clear",
                Style::default().fg(Color::White),
            ),
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
        if let Some(sub) = view.privacy_sub.take() {
            return self.handle_privacy_sub_key(sub, key);
        }
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
            KeyCode::Enter if view.row == 1 => {
                view.privacy_sub = Some(PrivacySub::Redaction {
                    values: provider::load_redaction_values()?,
                    row: 0,
                    reveal: false,
                    adding: None,
                });
            }
            KeyCode::Enter if view.row == 2 => {
                view.privacy_sub = Some(PrivacySub::Acknowledgements { row: 0 });
            }
            KeyCode::Char('c') if view.row == 1 => view.confirm_delete = true,
            KeyCode::Char('r') if view.row == 2 => view.confirm_delete = true,
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

#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum PrivacySub {
    Redaction {
        values: Vec<String>,
        row: usize,
        reveal: bool,
        adding: Option<String>,
    },
    Acknowledgements {
        row: usize,
    },
}

pub(super) fn mask_value(value: &str) -> String {
    let count = value.chars().count();
    if count <= 2 {
        return "•".repeat(count);
    }
    value.chars().take(2).collect::<String>() + &"•".repeat(count - 2)
}

const ACK_ROWS: usize = PRIVACY_FAMILIES.len() * 2;

fn ack_target(row: usize) -> (&'static str, bool) {
    (PRIVACY_FAMILIES[row / 2], row % 2 == 1)
}

fn toggle(list: &mut Vec<String>, family: &str) {
    if let Some(position) = list.iter().position(|item| item == family) {
        list.remove(position);
    } else {
        list.push(family.to_owned());
    }
}

impl App {
    fn handle_privacy_sub_key(&mut self, mut sub: PrivacySub, key: event::KeyEvent) -> Result<()> {
        let confirming = self
            .settings_view
            .as_ref()
            .is_some_and(|view| view.confirm_delete);
        let keep = match &mut sub {
            PrivacySub::Acknowledgements { row } => match key.code {
                KeyCode::Esc => false,
                KeyCode::Up => {
                    *row = row.saturating_sub(1);
                    true
                }
                KeyCode::Down => {
                    *row = (*row + 1).min(ACK_ROWS - 1);
                    true
                }
                KeyCode::Char(' ') | KeyCode::Enter => {
                    let (family, image) = ack_target(*row);
                    let list = if image {
                        &mut self.settings.privacy_image_acknowledged
                    } else {
                        &mut self.settings.privacy_acknowledged
                    };
                    toggle(list, family);
                    write_settings(&self.settings)?;
                    true
                }
                _ => true,
            },
            PrivacySub::Redaction {
                values,
                row,
                reveal,
                adding,
            } => {
                if confirming {
                    if let Some(view) = self.settings_view.as_mut() {
                        view.confirm_delete = false;
                    }
                    if key.code == KeyCode::Char('y') && *row < values.len() {
                        values.remove(*row);
                        provider::save_redaction_values(values)?;
                        *row = (*row).min(values.len().saturating_sub(1));
                        self.notice = "Redaction value removed.".to_owned();
                    }
                    true
                } else if let Some(text) = adding {
                    match key.code {
                        KeyCode::Esc => *adding = None,
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        KeyCode::Char(c) => text.push(c),
                        KeyCode::Enter => {
                            let value = text.trim().to_owned();
                            if !value.is_empty() && !values.contains(&value) {
                                values.push(value);
                                provider::save_redaction_values(values)?;
                                self.notice = "Redaction value added.".to_owned();
                            }
                            *adding = None;
                        }
                        _ => {}
                    }
                    true
                } else {
                    match key.code {
                        KeyCode::Esc => false,
                        KeyCode::Up => {
                            *row = row.saturating_sub(1);
                            true
                        }
                        KeyCode::Down => {
                            *row = (*row + 1).min(values.len());
                            true
                        }
                        KeyCode::Char('v') => {
                            *reveal = !*reveal;
                            true
                        }
                        KeyCode::Char('n') => {
                            *adding = Some(String::new());
                            true
                        }
                        KeyCode::Enter if *row == values.len() => {
                            *adding = Some(String::new());
                            true
                        }
                        KeyCode::Char('x') if *row < values.len() => {
                            if let Some(view) = self.settings_view.as_mut() {
                                view.confirm_delete = true;
                            }
                            true
                        }
                        _ => true,
                    }
                }
            }
        };
        if let Some(view) = self.settings_view.as_mut() {
            view.privacy_sub = keep.then_some(sub);
        }
        Ok(())
    }
}

pub(super) fn draw_privacy_sub(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    sub: &PrivacySub,
) {
    let accent = crate::tui::theme::accent();
    let pointer = |selected: bool| {
        Span::styled(
            if selected { "▸ " } else { "  " },
            Style::default().fg(accent),
        )
    };
    let mut lines = Vec::new();
    match sub {
        PrivacySub::Acknowledgements { row } => {
            lines.push(Line::from(Span::styled(
                "Unchecking makes the warning or image prompt appear again.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));
            for index in 0..ACK_ROWS {
                let (family, image) = ack_target(index);
                let list = if image {
                    &app.settings.privacy_image_acknowledged
                } else {
                    &app.settings.privacy_acknowledged
                };
                let checked = list.iter().any(|item| item == family);
                lines.push(Line::from(vec![
                    pointer(index == *row),
                    Span::styled(
                        if checked { "[x] " } else { "[ ] " },
                        Style::default().fg(if checked {
                            Color::Rgb(110, 220, 130)
                        } else {
                            Color::Gray
                        }),
                    ),
                    Span::styled(format!("{family:<15}"), Style::default().fg(Color::White)),
                    Span::styled(
                        if image {
                            "image content allowed"
                        } else {
                            "first-use warning acknowledged"
                        },
                        Style::default().fg(Color::Gray),
                    ),
                ]));
            }
        }
        PrivacySub::Redaction {
            values,
            row,
            reveal,
            adding,
        } => {
            lines.push(Line::from(Span::styled(
                "Values are replaced locally before text is sent to flagged providers.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));
            for (index, value) in values.iter().enumerate() {
                let shown = if *reveal {
                    value.clone()
                } else {
                    mask_value(value)
                };
                lines.push(Line::from(vec![
                    pointer(index == *row && adding.is_none()),
                    Span::styled(shown, Style::default().fg(Color::White)),
                ]));
            }
            match adding {
                Some(text) => lines.push(Line::from(vec![
                    pointer(true),
                    Span::styled("New value: ", Style::default().fg(Color::Gray)),
                    Span::styled(text.clone(), Style::default().fg(Color::White)),
                    Span::styled("_", Style::default().fg(accent)),
                ])),
                None => lines.push(Line::from(vec![
                    pointer(*row == values.len()),
                    Span::styled("+ Add value", Style::default().fg(Color::Gray)),
                ])),
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn privacy_sub_hint(sub: &PrivacySub) -> &'static str {
    match sub {
        PrivacySub::Acknowledgements { .. } => "↑↓ move   Space toggle   Esc back",
        PrivacySub::Redaction {
            adding: Some(_), ..
        } => "Enter save   Esc cancel",
        PrivacySub::Redaction { .. } => "↑↓ move   n add   x remove   v reveal/hide   Esc back",
    }
}

#[cfg(test)]
mod tests {
    use super::{PrivacySub, mask_value};
    use crate::tui::render::draw;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{Settings, provider};
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
        press(&mut app, KeyCode::Char('r'));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.settings.privacy_acknowledged.len(), 1);
        press(&mut app, KeyCode::Char('r'));
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
        assert!(text.contains("c to clear"), "{text}");
    }

    fn privacy_row(row: usize) -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.open_settings(Section::Privacy);
        press(&mut app, KeyCode::Right);
        for _ in 0..row {
            press(&mut app, KeyCode::Down);
        }
        app
    }

    fn sub(app: &App) -> Option<PrivacySub> {
        app.settings_view
            .as_ref()
            .and_then(|view| view.privacy_sub.clone())
    }

    #[test]
    fn mask_keeps_two_characters_and_hides_the_rest() {
        assert_eq!(mask_value("example-secret"), "ex••••••••••••");
        assert_eq!(mask_value("ab"), "••");
    }

    // One test owns the credential-store entry so parallel tests cannot race on it.
    #[test]
    fn redaction_values_can_be_added_revealed_and_removed_with_confirmation() {
        provider::save_redaction_values(&[]).expect("reset");
        let mut app = privacy_row(1);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(sub(&app), Some(PrivacySub::Redaction { .. })));
        press(&mut app, KeyCode::Char('n'));
        for c in "example-secret".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            provider::load_redaction_values().expect("load"),
            vec!["example-secret"]
        );
        press(&mut app, KeyCode::Char('v'));
        assert!(matches!(
            sub(&app),
            Some(PrivacySub::Redaction { reveal: true, .. })
        ));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(provider::load_redaction_values().expect("load").len(), 1);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert!(provider::load_redaction_values().expect("load").is_empty());
    }

    #[test]
    fn acknowledgement_checkboxes_toggle_one_setting_each() {
        let mut app = privacy_row(2);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(
            sub(&app),
            Some(PrivacySub::Acknowledgements { .. })
        ));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.settings.privacy_acknowledged, vec!["Google/Gemini"]);
        assert!(app.settings.privacy_image_acknowledged.is_empty());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            app.settings.privacy_image_acknowledged,
            vec!["Google/Gemini"]
        );
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            app.settings.privacy_acknowledged,
            vec!["Google/Gemini", "GLM/Z.ai"]
        );
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.settings.privacy_acknowledged, vec!["GLM/Z.ai"]);
    }

    #[test]
    fn esc_returns_from_subview_to_privacy_rows() {
        let mut app = privacy_row(2);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert_eq!(sub(&app), None);
        assert_eq!(app.settings_view.as_ref().map(|view| view.row), Some(2));
        assert_eq!(
            app.settings_view.as_ref().map(|view| view.section),
            Some(Section::Privacy)
        );
    }
}
