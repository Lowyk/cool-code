mod dialogs;
pub(super) mod settings;

use crate::policy::mode_label;
use crate::tui::effort::{draw_effort_picker, effort_name, effort_style, gradient_name};
use crate::tui::models::selected_model_name;
use crate::tui::pickers::model::draw_model_picker;
use crate::tui::render::dialogs::{
    draw_extreme_confirmation, draw_mode_picker, draw_model_provider_picker,
    draw_privacy_confirmation, draw_tool_approval, draw_workspace_trust_prompt,
};
use crate::tui::settings::draw_settings_view;
use crate::tui::state::{App, TranscriptKind};
use crate::tui::wordmark::cool_code_wordmark;
use crate::{Effort, provider};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

fn mode_color(mode: &str) -> Color {
    match mode {
        "accept-everything" => Color::Rgb(235, 80, 80),
        "accept-edits" => Color::Rgb(180, 130, 255),
        "auto" => Color::Rgb(110, 220, 130),
        "plan" => Color::Rgb(240, 210, 90),
        "accept-minimal" => Color::Rgb(98, 213, 244),
        _ => Color::Gray,
    }
}

pub(super) fn mode_span(mode: &str, selected: bool) -> Span<'static> {
    let label = mode_label(mode);
    let text = if mode == "accept-everything" {
        format!("!! {label} !!")
    } else {
        label.to_owned()
    };
    let color = mode_color(mode);
    let style = if selected {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color).add_modifier(Modifier::DIM)
    };
    Span::styled(text, style)
}

pub(super) fn wrap_input_text(input: &str, width: u16) -> (Vec<String>, (usize, usize)) {
    use unicode_width::UnicodeWidthChar as _;

    let width = width.max(1) as usize;
    let mut lines = vec![String::new()];
    let mut row = 0usize;
    let mut column = 2usize.min(width);
    let characters = input.chars().collect::<Vec<_>>();
    for (index, character) in characters.iter().copied().enumerate() {
        let character_width = character.width().unwrap_or(0);
        if column + character_width > width {
            lines.push(String::new());
            row += 1;
            column = 0;
        }
        lines[row].push(character);
        column += character_width;
        if index + 1 == characters.len() && column == width {
            lines.push(String::new());
            row += 1;
            column = 0;
        }
    }
    (lines, (row, column))
}

pub(super) fn input_visual_lines(input: &str, width: u16) -> usize {
    wrap_input_text(input, width).0.len()
}

pub(super) fn input_prompt_height(input: &str, area: Rect) -> u16 {
    let prompt_width = centered_rect(78, 100, Rect::new(area.x, area.y, area.width, 1))
        .width
        .saturating_sub(3);
    let needed = input_visual_lines(input, prompt_width).saturating_add(1);
    let available = area.height.saturating_sub(12).clamp(4, 10) as usize;
    needed.clamp(4, available) as u16
}

pub(super) fn draw(frame: &mut ratatui::Frame<'_>, app: &App, animation_tick: usize) {
    let area = frame.area();
    let prompt_height = input_prompt_height(&app.input, area);
    let (logo_area, subtitle_area, history_area, prompt_area, help_area, status_area) =
        if app.transcript.is_empty() {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Fill(1),
                    Constraint::Length(5),
                    Constraint::Length(2),
                    Constraint::Length(prompt_height),
                    Constraint::Length(1),
                    Constraint::Length(2),
                    Constraint::Fill(1),
                    Constraint::Length(1),
                ])
                .split(area);
            (
                layout[1],
                layout[2],
                Rect::default(),
                layout[3],
                layout[5],
                layout[7],
            )
        } else {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Fill(5),
                    Constraint::Length(prompt_height),
                    Constraint::Length(2),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(area);
            (
                Rect::default(),
                Rect::default(),
                layout[1],
                layout[2],
                layout[3],
                layout[5],
            )
        };

    let logo_elapsed = app.launched_at.elapsed().as_secs_f32().min(1.0);
    let logo_lines = cool_code_wordmark(logo_elapsed);
    if logo_area.width > 0 {
        frame.render_widget(
            Paragraph::new(logo_lines).alignment(Alignment::Center),
            logo_area,
        );
        frame.render_widget(
            Paragraph::new("Rust · temperature-conscious coding harness")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            subtitle_area,
        );
    }

    if !app.transcript.is_empty() {
        let mut lines = Vec::new();
        for entry in &app.transcript {
            let (marker, color, content_color) = match entry.kind {
                TranscriptKind::User => ("> ", Color::Rgb(120, 220, 245), Color::White),
                TranscriptKind::Assistant => ("• ", Color::Rgb(165, 236, 250), Color::White),
                TranscriptKind::CommandOutput => {
                    ("  ", Color::Rgb(185, 195, 205), Color::Rgb(200, 205, 212))
                }
                TranscriptKind::Error => {
                    ("| ", Color::Rgb(255, 100, 110), Color::Rgb(255, 145, 150))
                }
            };
            let mut content_lines = entry.text.lines();
            lines.push(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    content_lines.next().unwrap_or_default(),
                    Style::default().fg(content_color),
                ),
            ]));
            for content_line in content_lines {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{}{content_line}",
                        if entry.kind == TranscriptKind::Error {
                            "| "
                        } else {
                            "  "
                        }
                    ),
                    Style::default().fg(if entry.kind == TranscriptKind::Error {
                        color
                    } else {
                        content_color
                    }),
                )));
            }
            lines.push(Line::from(""));
        }
        let wrapped_line_count = lines
            .iter()
            .map(|line| {
                let display_width = line
                    .spans
                    .iter()
                    .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
                    .sum::<usize>();
                display_width
                    .div_ceil(history_area.width.max(1) as usize)
                    .max(1)
            })
            .sum::<usize>();
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let max_scroll = wrapped_line_count.saturating_sub(history_area.height as usize) as u16;
        let scroll = max_scroll.saturating_sub(app.history_scroll.min(max_scroll));
        frame.render_widget(paragraph.scroll((scroll, 0)), history_area);
    }

    let prompt_area = centered_rect(78, 100, prompt_area);
    let prompt_block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::Rgb(98, 213, 244)))
        .style(Style::default().bg(Color::Rgb(37, 38, 40)))
        .padding(ratatui::widgets::Padding::new(2, 0, 1, 0));
    let prompt_inner = prompt_block.inner(prompt_area);
    let (input_lines, (cursor_line, cursor_column)) =
        wrap_input_text(&app.input, prompt_inner.width);
    let prompt = if app.input.is_empty() {
        vec![Line::from(vec![
            Span::styled("› ", Style::default().fg(Color::Rgb(98, 213, 244))),
            Span::styled(
                "Describe what you want to change…",
                Style::default().fg(Color::DarkGray),
            ),
        ])]
    } else {
        input_lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if index == 0 {
                    Line::from(vec![
                        Span::styled("› ", Style::default().fg(Color::Rgb(98, 213, 244))),
                        Span::styled(line.clone(), Style::default().fg(Color::White)),
                    ])
                } else {
                    Line::from(Span::styled(
                        line.clone(),
                        Style::default().fg(Color::White),
                    ))
                }
            })
            .collect()
    };
    let prompt_lines = prompt.len();
    let prompt_scroll = prompt_lines.saturating_sub(prompt_inner.height as usize) as u16;
    frame.render_widget(
        Paragraph::new(prompt)
            .scroll((prompt_scroll, 0))
            .block(prompt_block),
        prompt_area,
    );
    if !app.picker
        && !app.confirm_extreme
        && !app.trust_prompt
        && app.tool_approval.is_none()
        && app.privacy_confirmation.is_none()
    {
        let visible_line = cursor_line.saturating_sub(prompt_scroll as usize);
        frame.set_cursor_position(Position::new(
            (prompt_inner.x + cursor_column as u16).min(prompt_inner.right().saturating_sub(1)),
            (prompt_inner.y
                + visible_line.min(prompt_inner.height.saturating_sub(1) as usize) as u16)
                .min(prompt_inner.bottom().saturating_sub(1)),
        ));
    }

    let help = Line::from(vec![
        Span::styled(
            "Enter",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" submit   ", Style::default().fg(Color::DarkGray)),
        Span::styled("/settings", Style::default().fg(Color::Rgb(98, 213, 244))),
        Span::styled("  ", Style::default()),
        Span::styled("/effort", Style::default().fg(Color::Rgb(98, 213, 244))),
        Span::styled(
            "   /mode   /init   @path   Ctrl+↑/↓ scroll   Esc quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(help).alignment(Alignment::Center), help_area);

    let model = app
        .settings
        .model
        .as_deref()
        .map(|id| selected_model_name(&app.settings, id))
        .unwrap_or_else(|| "no model selected".to_owned());
    let effort_is_flashing = app
        .effort_flash_until
        .is_some_and(|until| std::time::Instant::now() < until);
    let effort_spans = if effort_is_flashing
        && matches!(
            app.settings.effort,
            Effort::Max | Effort::XHigh | Effort::Super | Effort::Extreme
        ) {
        gradient_name(app.settings.effort, true, animation_tick)
    } else {
        vec![Span::styled(
            effort_name(app.settings.effort),
            effort_style(app.settings.effort, effort_is_flashing),
        )]
    };
    let mut status_spans = vec![
        mode_span(&app.settings.permission_mode, true),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(model, Style::default().fg(Color::White)),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
    ];
    status_spans.extend(effort_spans);
    status_spans.extend([
        Span::styled("  ", Style::default()),
        Span::styled(&app.notice, Style::default().fg(Color::DarkGray)),
    ]);
    let status = Line::from(status_spans);
    frame.render_widget(
        Paragraph::new(status)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        status_area,
    );

    if app.settings_view.is_some() {
        draw_settings_view(frame, area, app);
    }
    if app.picker {
        draw_effort_picker(frame, area, app, animation_tick);
    }
    if app.confirm_extreme {
        draw_extreme_confirmation(frame, area);
    }
    if let Some(prompt) = app.privacy_confirmation.as_ref() {
        let has_image = app
            .pending_privacy_message
            .as_ref()
            .is_some_and(provider::message_contains_image);
        draw_privacy_confirmation(frame, area, prompt, has_image);
    }
    if app.mode_picker {
        draw_mode_picker(frame, area, app);
    }
    if app.model_choices.is_some() {
        draw_model_provider_picker(frame, area, app);
    }
    if let Some(picker) = app.model_picker.as_ref() {
        draw_model_picker(frame, area, picker);
    }
    if app.trust_prompt {
        draw_workspace_trust_prompt(frame, area, app);
    }
    if let Some(approval) = app.tool_approval.as_ref() {
        draw_tool_approval(frame, area, approval, app.approval_scroll);
    }
}

pub(super) fn centered_rect(width_percent: u16, height_percent: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::{draw, input_prompt_height, input_visual_lines, mode_span, wrap_input_text};
    use crate::Settings;
    use crate::tui::state::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};

    const MODE_COLORS: [(&str, Color); 5] = [
        ("accept-everything", Color::Rgb(235, 80, 80)),
        ("accept-edits", Color::Rgb(180, 130, 255)),
        ("auto", Color::Rgb(110, 220, 130)),
        ("plan", Color::Rgb(240, 210, 90)),
        ("accept-minimal", Color::Rgb(98, 213, 244)),
    ];

    #[test]
    fn each_permission_mode_has_its_own_color() {
        for (mode, color) in MODE_COLORS {
            let span = mode_span(mode, true);
            assert_eq!(span.style.fg, Some(color), "{mode}");
            assert!(span.style.add_modifier.contains(Modifier::BOLD), "{mode}");
            assert!(
                !mode_span(mode, false)
                    .style
                    .add_modifier
                    .contains(Modifier::BOLD)
            );
        }
    }

    #[test]
    fn only_accept_everything_carries_warning_marks() {
        assert_eq!(
            mode_span("accept-everything", true).content,
            "!! Accept Everything !!"
        );
        for mode in ["accept-edits", "auto", "plan", "accept-minimal"] {
            assert!(!mode_span(mode, true).content.contains('!'), "{mode}");
        }
    }

    #[test]
    fn status_row_renders_the_mode_in_its_color() {
        for (mode, color) in MODE_COLORS {
            let mut settings = Settings::default();
            settings.permission_mode = mode.to_owned();
            let mut app = App::new(settings);
            app.trust_prompt = false;
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("test terminal");
            terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
            let buffer = terminal.backend().buffer();
            let found = buffer
                .content()
                .iter()
                .any(|cell| cell.fg == color && cell.symbol() != " ");
            assert!(found, "{mode} label not drawn in {color:?}");
        }
    }

    #[test]
    fn long_prompt_wraps_and_grows_the_input_area() {
        assert_eq!(input_visual_lines("short", 20), 1);
        assert_eq!(input_visual_lines(&"x".repeat(45), 20), 3);
        let area = Rect::new(0, 0, 100, 30);
        assert!(input_prompt_height(&"x".repeat(300), area) > 4);
    }

    #[test]
    fn prompt_cursor_tracks_the_explicit_continuation_lines() {
        let (lines, cursor) = wrap_input_text("abcdefghij", 10);
        assert_eq!(lines, ["abcdefgh", "ij"]);
        assert_eq!(cursor, (1, 2));

        let (wide_lines, wide_cursor) = wrap_input_text("ab界c", 6);
        assert_eq!(wide_lines, ["ab界", "c"]);
        assert_eq!(wide_cursor, (1, 1));
    }
}
