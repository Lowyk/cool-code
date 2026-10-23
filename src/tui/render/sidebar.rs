//! The column beside a conversation: the session's name, how much of the context is used, the
//! mode and model, the files the model changed and the extensions that are running.

use super::mode_span;
use crate::tui::state::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

/// Columns the sidebar takes.
pub(super) const WIDTH: u16 = 32;
/// The narrowest window that has room for it next to a conversation.
const MIN_WINDOW: u16 = 110;
/// Most changed files listed.
const MAX_FILES: usize = 8;

/// Whether a window `width` columns wide shows the sidebar.
pub(super) fn shown(app: &App, width: u16) -> bool {
    app.settings.sidebar && !app.transcript.is_empty() && width >= MIN_WINDOW
}

pub(super) fn draw_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    if area.width < 8 || area.height == 0 {
        return;
    }
    let buffer = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buffer[(x, y)].set_bg(crate::tui::theme::panel());
        }
    }
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width - 3,
        area.height.saturating_sub(1),
    );
    let heading = |text: &str| {
        Line::from(Span::styled(
            text.to_owned(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let dim = Style::default().fg(Color::Gray);
    let faint = Style::default().fg(Color::DarkGray);
    let mut lines = vec![heading(&app.session_title()), Line::from("")];
    lines.push(heading("Context"));
    match app.context_figures() {
        Some((used, window)) => {
            lines.push(Line::from(Span::styled(
                format!("{} tokens", grouped(used)),
                dim,
            )));
            if let Some(window) = window.filter(|window| *window > 0) {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{}% used of {}",
                        (used * 100 + window / 2) / window,
                        grouped(window)
                    ),
                    dim,
                )));
            }
        }
        None => lines.push(Line::from(Span::styled("nothing yet", faint))),
    }
    lines.push(Line::from(""));
    lines.push(heading("Session"));
    lines.push(Line::from(mode_span(&app.settings.permission_mode, false)));
    let model = app
        .settings
        .model
        .as_deref()
        .map(|id| crate::tui::models::selected_model_name(&app.settings, id))
        .unwrap_or_else(|| "no model selected".to_owned());
    lines.push(Line::from(Span::styled(model, dim)));
    lines.push(Line::from(Span::styled(
        format!(
            "effort {}",
            crate::tui::effort::effort_name(app.settings.effort)
        ),
        faint,
    )));
    let files = app.changed_files();
    if !files.is_empty() {
        lines.push(Line::from(""));
        lines.push(heading("Files changed"));
        for name in files.iter().take(MAX_FILES) {
            lines.push(Line::from(Span::styled(name.clone(), dim)));
        }
        if files.len() > MAX_FILES {
            lines.push(Line::from(Span::styled(
                format!("and {} more", files.len() - MAX_FILES),
                faint,
            )));
        }
    }
    let servers = crate::extensions::mcp_hub::hub().statuses();
    if !servers.is_empty() {
        lines.push(Line::from(""));
        lines.push(heading("MCP servers"));
        for server in servers {
            let (mark, color) = match server.phase {
                crate::extensions::mcp_hub::Phase::Ready => ("●", Color::Rgb(110, 220, 130)),
                crate::extensions::mcp_hub::Phase::Starting => ("○", Color::Gray),
                crate::extensions::mcp_hub::Phase::Failed(_) => ("●", Color::Rgb(235, 80, 80)),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{mark} "), Style::default().fg(color)),
                Span::styled(server.id, dim),
            ]));
        }
    }
    let statuses = app.extensions.mods.statuses();
    if !statuses.is_empty() {
        lines.push(Line::from(""));
        lines.push(heading("Mods"));
        for text in statuses.into_iter().take(3) {
            lines.push(Line::from(Span::styled(text, dim)));
        }
    }
    // The folder sits on the last row, whatever else is shown.
    let body = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);
    if inner.height >= 2 {
        frame.render_widget(
            Paragraph::new(Span::styled(app.folder.as_str(), faint)),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
}

/// `7966` as `7,966`.
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::grouped;

    #[test]
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(7_966), "7,966");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
