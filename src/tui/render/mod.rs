mod dialogs;
pub(super) mod forms;
mod motion;

use crate::policy::mode_label;
use crate::tui::backdrop::{backdrop_enabled, draw_backdrop};
use crate::tui::effort::draw_effort_picker;
use crate::tui::models::selected_model_name;
use crate::tui::pickers::model::draw_model_picker;
use crate::tui::render::dialogs::{
    draw_mode_picker, draw_model_provider_picker, draw_privacy_confirmation,
    draw_ultimate_confirmation, draw_workspace_trust_prompt,
};
pub(super) use crate::tui::render::dialogs::{privacy_dialog, trust_dialog, ultimate_dialog};
use crate::tui::render::motion::pulse_spans;
use crate::tui::settings::draw_settings_view;
use crate::tui::state::{App, StreamingTurn, TranscriptKind};
use crate::tui::stats_view::draw_stats;
use crate::tui::wordmark::{cool_code_wordmark, tagline_lines, wordmark_height};
use crate::{PulseMode, provider};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
        "manual" => Color::Rgb(255, 150, 90),
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

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn status_line(turn: &StreamingTurn, now: std::time::Instant) -> String {
    let elapsed = now.saturating_duration_since(turn.started);
    let spinner = SPINNER[(elapsed.as_millis() / 80) as usize % SPINNER.len()];
    if let Some((label, started)) = &turn.tool {
        let seconds = now.saturating_duration_since(*started).as_secs_f32();
        return format!("{spinner} {label} · {seconds:.1}s · Esc to cancel");
    }
    let tokens = match turn.usage {
        Some(tokens) => format!("{tokens} tokens"),
        // Rough estimate when the provider does not report usage while streaming.
        None => format!("~{} tokens", turn.text.chars().count() / 4),
    };
    format!(
        "{spinner} {:.1}s · {tokens} · Esc to cancel",
        elapsed.as_secs_f32()
    )
}

/// Puts `marker` before the first line of an entry and an indent before the rest.
fn marked(marker: Span<'static>, body: Vec<Line<'static>>) -> Vec<Line<'static>> {
    if body.is_empty() {
        return vec![Line::from(marker)];
    }
    body.into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            let lead = if index == 0 {
                marker.clone()
            } else {
                Span::raw("  ")
            };
            line.spans.insert(0, lead);
            line
        })
        .collect()
}

/// The answer being written. Lines that are complete are drawn as Markdown; the line in
/// progress is plain text with the pulse, and becomes Markdown when its line ends.
fn streaming_lines(turn: &StreamingTurn, mode: PulseMode) -> Vec<Line<'static>> {
    let marker = Span::styled(
        "• ",
        Style::default()
            .fg(crate::tui::theme::accent_soft())
            .add_modifier(Modifier::BOLD),
    );
    let (done, current) = match turn.text.rfind('\n') {
        Some(index) => turn.text.split_at(index + 1),
        None => ("", turn.text.as_str()),
    };
    let mut lines = if done.trim().is_empty() {
        Vec::new()
    } else {
        crate::tui::markdown::render(done)
    };
    let arrivals: Vec<(usize, std::time::Instant)> = turn
        .arrivals
        .iter()
        .map(|(offset, at)| (offset.saturating_sub(done.len()), *at))
        .collect();
    let mut tail = pulse_spans(current, &arrivals, mode, std::time::Instant::now());
    tail.push(Span::styled(
        "▍",
        Style::default().fg(crate::tui::theme::accent()),
    ));
    lines.push(Line::from(tail));
    marked(marker, lines)
}

pub(super) fn wrap_input_text(input: &str, width: u16) -> (Vec<String>, (usize, usize)) {
    use unicode_width::UnicodeWidthChar as _;

    let width = width.max(1) as usize;
    let mut lines = vec![String::new()];
    let mut row = 0usize;
    let mut column = 2usize.min(width);
    let characters = input.chars().collect::<Vec<_>>();
    for (index, character) in characters.iter().copied().enumerate() {
        if character == '\n' {
            lines.push(String::new());
            row += 1;
            column = 0;
            continue;
        }
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

/// How strongly the backdrop shows behind a conversation when dimming is on.
const CHAT_BACKDROP_DIM: f32 = 0.4;

pub(super) fn draw(frame: &mut ratatui::Frame<'_>, app: &App, animation_tick: usize) {
    draw_dark(frame, app, animation_tick);
    if app.settings.light_mode {
        crate::tui::theme::to_light(frame.buffer_mut());
    }
}

fn draw_dark(frame: &mut ratatui::Frame<'_>, app: &App, animation_tick: usize) {
    let area = frame.area();
    crate::tui::theme::set_current(app.settings.theme);
    app.cursor.set(None);
    app.hits.clear();
    // The wheel scrolls the conversation unless something drawn later is in the way; a window
    // that takes the keys also takes the mouse.
    app.hits.wheel(
        area,
        KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL),
    );
    if !app.typing_in_prompt() {
        app.hits.modal(area);
    }
    if let Some(background) = crate::tui::theme::current().screen_bg {
        frame.render_widget(
            ratatui::widgets::Block::default().style(Style::default().bg(background)),
            area,
        );
    }
    let prompt_height = input_prompt_height(&app.input, area);
    let (logo_area, subtitle_area, history_area, prompt_area, help_area, notice_area, status_area) =
        if app.transcript.is_empty() {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Fill(1),
                    Constraint::Length(wordmark_height(area.width)),
                    Constraint::Length(3),
                    Constraint::Length(prompt_height),
                    Constraint::Length(1),
                    Constraint::Length(2),
                    Constraint::Fill(1),
                    Constraint::Length(2),
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
                layout[8],
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
                    Constraint::Length(2),
                    Constraint::Length(1),
                ])
                .split(area);
            (
                Rect::default(),
                Rect::default(),
                layout[1],
                layout[2],
                layout[3],
                layout[4],
                layout[5],
            )
        };

    let no_color = std::env::var_os("NO_COLOR").is_some();
    // The backdrop fills the welcome screen; behind a conversation it is opt-in and dimmed.
    let chatting = !app.transcript.is_empty();
    let wanted = if chatting {
        app.settings.backdrop_in_chat
    } else {
        app.settings.background_animation
    };
    if backdrop_enabled(wanted, no_color) {
        let dim = if chatting && app.settings.dim_backdrop_in_chat {
            CHAT_BACKDROP_DIM
        } else {
            1.0
        };
        draw_backdrop(
            frame,
            area,
            app.launched_at.elapsed().as_secs_f32(),
            crate::tui::theme::current(),
            dim,
        );
    }

    let launched = app.launched_at.elapsed().as_secs_f32();
    if logo_area.width > 0 {
        frame.render_widget(
            Paragraph::new(cool_code_wordmark(launched, logo_area.width))
                .alignment(Alignment::Center),
            logo_area,
        );
        frame.render_widget(
            Paragraph::new(tagline_lines(launched)).alignment(Alignment::Center),
            subtitle_area,
        );
    }

    if !app.transcript.is_empty() {
        let mut lines = Vec::new();
        for entry in &app.transcript {
            let (marker, color, content_color) = match entry.kind {
                TranscriptKind::User => ("> ", crate::tui::theme::accent_bright(), Color::White),
                TranscriptKind::Assistant => ("• ", crate::tui::theme::accent_soft(), Color::White),
                TranscriptKind::CommandOutput => {
                    ("  ", Color::Rgb(185, 195, 205), Color::Rgb(200, 205, 212))
                }
                TranscriptKind::Error => {
                    ("| ", Color::Rgb(255, 100, 110), Color::Rgb(255, 145, 150))
                }
            };
            if entry.kind == TranscriptKind::Assistant {
                let marker = Span::styled(
                    marker,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                );
                lines.extend(marked(marker, crate::tui::markdown::render(&entry.text)));
                lines.push(Line::from(""));
                continue;
            }
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
        if let Some(turn) = app.streaming.as_ref()
            && turn.tool.is_none()
        {
            lines.extend(streaming_lines(turn, app.settings.pulse));
        }
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        // Exact count from the same word wrapping the paragraph draws with; estimating it
        // from character widths under-counts and hides the end of long answers.
        let wrapped_line_count = paragraph.line_count(history_area.width.max(1));
        let max_scroll = wrapped_line_count
            .saturating_sub(history_area.height as usize)
            .min(u16::MAX as usize) as u16;
        let scroll = max_scroll.saturating_sub(app.history_scroll.min(max_scroll));
        frame.render_widget(paragraph.scroll((scroll, 0)), history_area);
    }

    let prompt_area = centered_rect(78, 100, prompt_area);
    let prompt_block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(crate::tui::theme::accent()))
        .style(Style::default().bg(crate::tui::theme::input()))
        .padding(ratatui::widgets::Padding::new(2, 0, 1, 0));
    let prompt_inner = prompt_block.inner(prompt_area);
    let (input_lines, (cursor_line, cursor_column)) =
        wrap_input_text(&app.input, prompt_inner.width);
    let prompt = if app.input.is_empty() {
        vec![Line::from(vec![
            Span::styled("› ", Style::default().fg(crate::tui::theme::accent())),
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
                        Span::styled("› ", Style::default().fg(crate::tui::theme::accent())),
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
        && !app.confirm_ultimate
        && !app.trust_prompt
        && app.tool_approval.is_none()
        && app.privacy_confirmation.is_none()
    {
        let visible_line = cursor_line.saturating_sub(prompt_scroll as usize);
        let position = Position::new(
            (prompt_inner.x + cursor_column as u16).min(prompt_inner.right().saturating_sub(1)),
            (prompt_inner.y
                + visible_line.min(prompt_inner.height.saturating_sub(1) as usize) as u16)
                .min(prompt_inner.bottom().saturating_sub(1)),
        );
        frame.set_cursor_position(position);
        app.cursor.set(Some(position));
    }

    if let Some(state) = app.mention.as_ref() {
        crate::tui::mentions::draw_mentions(frame, prompt_area, state, &app.hits);
    }

    let help = Line::from(vec![
        Span::styled(
            "Enter",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" submit   ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "/settings",
            Style::default().fg(crate::tui::theme::accent()),
        ),
        Span::styled("  ", Style::default()),
        Span::styled("/effort", Style::default().fg(crate::tui::theme::accent())),
        Span::styled(
            "   /mode   /init   @path   Ctrl+↑/↓ scroll   Esc quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let help = match app.streaming.as_ref() {
        Some(turn) => Line::from(Span::styled(
            status_line(turn, std::time::Instant::now()),
            Style::default().fg(Color::Rgb(120, 170, 200)),
        )),
        None => help,
    };
    frame.render_widget(Paragraph::new(help).alignment(Alignment::Center), help_area);

    let model = app
        .settings
        .model
        .as_deref()
        .map(|id| selected_model_name(&app.settings, id))
        .unwrap_or_else(|| "no model selected".to_owned());
    let effort_spans =
        crate::tui::effort::status_effort_spans(app, animation_tick, std::time::Instant::now());
    let mut status_spans = vec![
        mode_span(&app.settings.permission_mode, true),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(model, Style::default().fg(Color::White)),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
    ];
    status_spans.extend(effort_spans);
    if let Some((text, urgency)) = app.context_status() {
        let color = match urgency {
            0 => Color::DarkGray,
            1 => Color::Rgb(255, 197, 92),
            _ => Color::Rgb(235, 80, 80),
        };
        status_spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
        status_spans.push(Span::styled(text, Style::default().fg(color)));
    }
    if let Some(warning) = app.usage_warning.as_ref() {
        let color = match warning.severity {
            crate::tui::usage_warnings::Severity::Low => Color::Rgb(255, 197, 92),
            crate::tui::usage_warnings::Severity::Critical => Color::Rgb(235, 80, 80),
        };
        status_spans.push(Span::raw("  "));
        status_spans.push(Span::styled(
            format!("!! {} !!", warning.short),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    let status = Line::from(status_spans);
    // The one-line message about what just happened sits above the status line.
    frame.render_widget(
        Paragraph::new(Span::styled(
            app.notice.as_str(),
            Style::default().fg(Color::DarkGray),
        ))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        notice_area,
    );
    frame.render_widget(
        Paragraph::new(status)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        status_area,
    );

    if app.settings_view.is_some() {
        draw_settings_view(frame, area, app);
    }
    if app.stats_view.is_some() {
        draw_stats(frame, area, app);
    }
    if app.usage_view.is_some() {
        crate::tui::usage_view::draw_usage(frame, area, app);
    }
    if app.picker {
        draw_effort_picker(frame, area, app, animation_tick);
    }
    if app.confirm_ultimate {
        draw_ultimate_confirmation(frame, area, app);
    }
    if let Some(prompt) = app.privacy_confirmation.as_ref() {
        let has_image = app
            .pending_privacy_message
            .as_ref()
            .is_some_and(provider::message_contains_image);
        draw_privacy_confirmation(frame, area, app, prompt, has_image);
    }
    if app.mode_picker {
        draw_mode_picker(frame, area, app);
    }
    if app.model_choices.is_some() {
        draw_model_provider_picker(frame, area, app);
    }
    if let Some(picker) = app.session_picker.as_ref() {
        crate::tui::sessions::draw_session_picker(frame, area, app, picker);
    }
    if let Some(login) = app.chatgpt_login.as_ref() {
        crate::tui::chatgpt_login::draw_chatgpt_login(frame, area, login);
    }
    if let Some(prompt) = app.outside_prompt.as_ref() {
        crate::tui::mentions::draw_outside_prompt(
            frame,
            area,
            prompt,
            &app.dialog_focus,
            &app.hits,
        );
    }
    if let Some(setup) = app.image_setup.as_ref() {
        crate::tui::image_setup::draw_image_setup(frame, area, setup);
    }
    if let Some(picker) = app.model_picker.as_ref() {
        draw_model_picker(frame, area, picker, app);
    }
    if let Some(wizard) = app.wizard.as_ref()
        && !app.trust_prompt
    {
        crate::tui::setup::draw_setup(frame, area, wizard, &app.hits);
    }
    if app.trust_prompt {
        draw_workspace_trust_prompt(frame, area, app);
    }
    if let Some(approval) = app.tool_approval.as_ref() {
        crate::tui::approval::draw_approval_card(frame, area, prompt_area, app, approval);
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

    const MODE_COLORS: [(&str, Color); 6] = [
        ("accept-everything", Color::Rgb(235, 80, 80)),
        ("accept-edits", Color::Rgb(180, 130, 255)),
        ("auto", Color::Rgb(110, 220, 130)),
        ("plan", Color::Rgb(240, 210, 90)),
        ("accept-minimal", Color::Rgb(98, 213, 244)),
        ("manual", Color::Rgb(255, 150, 90)),
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
        for mode in ["accept-edits", "auto", "plan", "accept-minimal", "manual"] {
            assert!(!mode_span(mode, true).content.contains('!'), "{mode}");
        }
    }

    #[test]
    fn status_row_renders_the_mode_in_its_color() {
        for (mode, color) in MODE_COLORS {
            let mut settings = Settings::default();
            settings.permission_mode = mode.to_owned();
            settings.providers = vec![crate::ProviderProfile {
                id: "p".to_owned(),
                name: "p".to_owned(),
                adapter: "openai-compatible".to_owned(),
                ..Default::default()
            }];
            settings.auto_guards = vec![crate::guard::AutoGuard {
                provider_id: "p".to_owned(),
                model_id: "haiku".to_owned(),
            }];
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

#[cfg(test)]
mod backdrop_tests {
    use super::draw;
    use crate::Settings;
    use crate::tui::backdrop::PARTICLE_GLYPHS;
    use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn particle_count(app: &App) -> usize {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| PARTICLE_GLYPHS.contains(&cell.symbol()))
            .count()
    }

    fn app(background: bool) -> App {
        let mut settings = Settings::default();
        settings.background_animation = background;
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app
    }

    #[test]
    fn welcome_screen_shows_the_backdrop() {
        assert!(particle_count(&app(true)) > 10);
    }

    #[test]
    fn backdrop_stays_off_during_a_conversation() {
        let mut app = app(true);
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: "hello".to_owned(),
        });
        assert_eq!(particle_count(&app), 0);
    }

    fn chatting(settings: impl FnOnce(&mut Settings)) -> App {
        let mut app = app(true);
        settings(&mut app.settings);
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Assistant,
            text: "hello".to_owned(),
        });
        app
    }

    #[test]
    fn the_chat_backdrop_is_off_until_asked_for() {
        assert_eq!(particle_count(&chatting(|_| {})), 0);
        assert!(particle_count(&chatting(|s| s.backdrop_in_chat = true)) > 5);
    }

    #[test]
    fn the_chat_backdrop_ignores_the_welcome_switch() {
        let on_in_chat_only = chatting(|s| {
            s.background_animation = false;
            s.backdrop_in_chat = true;
        });
        assert!(particle_count(&on_in_chat_only) > 5);
        let welcome_only = chatting(|s| {
            s.background_animation = true;
            s.backdrop_in_chat = false;
        });
        assert_eq!(particle_count(&welcome_only), 0);
    }

    fn glyph_strength(app: &App) -> u32 {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| PARTICLE_GLYPHS.contains(&cell.symbol()))
            .map(|cell| match cell.fg {
                ratatui::style::Color::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b),
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn dimming_only_affects_the_conversation_not_the_welcome_screen() {
        let bright = chatting(|s| {
            s.backdrop_in_chat = true;
            s.dim_backdrop_in_chat = false;
        });
        let dimmed = chatting(|s| {
            s.backdrop_in_chat = true;
            s.dim_backdrop_in_chat = true;
        });
        assert!(glyph_strength(&dimmed) * 10 < glyph_strength(&bright) * 7);
        // The welcome screen is never dimmed, whatever the setting says.
        let mut welcome_dim = app(true);
        welcome_dim.settings.dim_backdrop_in_chat = true;
        let mut welcome_bright = app(true);
        welcome_bright.settings.dim_backdrop_in_chat = false;
        // The animation clock moves between the two draws, so allow a little drift.
        let (a, b) = (
            glyph_strength(&welcome_dim),
            glyph_strength(&welcome_bright),
        );
        assert!(a.abs_diff(b) * 50 < a, "{a} vs {b}");
    }

    #[test]
    fn no_color_turns_off_the_chat_backdrop_too() {
        // NO_COLOR is read from the environment; the guard itself is what is under test.
        assert!(!crate::tui::backdrop::backdrop_enabled(true, true));
    }

    #[test]
    fn the_theme_backdrop_replaces_the_original_one() {
        let mut app = app(true);
        app.settings.theme = crate::ThemeId::Sakura;
        assert_eq!(particle_count(&app), 0, "snow glyphs are gone");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        let petals = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| ["✿", "❀"].contains(&cell.symbol()))
            .count();
        assert!(petals > 0, "petals fall on the Sakura welcome screen");
    }

    #[test]
    fn backdrop_setting_turns_it_off() {
        assert_eq!(particle_count(&app(false)), 0);
    }
}

#[cfg(test)]
mod notice_tests {
    use super::draw;
    use crate::Settings;
    use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rows(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    fn row_of(rows: &[String], needle: &str) -> usize {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| {
                panic!(
                    "{needle:?} not on screen:
{}",
                    rows.join(
                        "
"
                    )
                )
            })
    }

    fn app(conversation: bool) -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.notice = "Conversation cleared.".to_owned();
        if conversation {
            app.transcript.push(TranscriptEntry {
                kind: TranscriptKind::User,
                text: "hello".to_owned(),
            });
        }
        app
    }

    #[test]
    fn the_notice_sits_above_the_status_line_not_beside_it() {
        for conversation in [false, true] {
            let rows = rows(&app(conversation), 100, 30);
            let notice = row_of(&rows, "Conversation cleared.");
            let status = row_of(&rows, "no model selected");
            assert!(
                notice < status,
                "conversation={conversation}: notice row {notice}, status row {status}"
            );
            assert!(
                !rows[status].contains("Conversation cleared."),
                "the status line no longer carries the notice"
            );
        }
    }

    #[test]
    fn a_long_notice_wraps_instead_of_pushing_the_status_off_screen() {
        let mut app = app(true);
        app.notice = "word ".repeat(60);
        let rows = rows(&app, 80, 24);
        assert!(rows.iter().any(|row| row.contains("no model selected")));
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::{draw, status_line};
    use crate::Settings;
    use crate::agent::PendingEvent;
    use crate::tui::state::{App, StreamingTurn};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn turn(text: &str, usage: Option<u64>, tool: Option<&str>) -> (StreamingTurn, Instant) {
        let mut turn = StreamingTurn::new(Arc::new(AtomicBool::new(false)));
        let now = turn.started + Duration::from_millis(4_200);
        turn.text = text.to_owned();
        turn.usage = usage;
        turn.tool = tool.map(|label| (label.to_owned(), turn.started));
        (turn, now)
    }

    #[test]
    fn status_line_uses_reported_usage() {
        let (turn, now) = turn("hello", Some(512), None);
        let line = status_line(&turn, now);
        assert!(
            line.ends_with("4.2s · 512 tokens · Esc to cancel"),
            "{line}"
        );
    }

    #[test]
    fn status_line_estimates_without_usage() {
        let (turn, now) = turn(&"x".repeat(40), None, None);
        assert!(status_line(&turn, now).contains("~10 tokens"));
    }

    #[test]
    fn status_line_shows_running_tool() {
        let (turn, now) = turn("", None, Some("cargo test"));
        let line = status_line(&turn, now);
        assert!(
            line.ends_with("cargo test · 4.2s · Esc to cancel"),
            "{line}"
        );
    }

    #[test]
    fn streaming_turn_renders_text_cursor_and_status() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.transcript.push(crate::tui::state::TranscriptEntry {
            kind: crate::tui::state::TranscriptKind::User,
            text: "hi".to_owned(),
        });
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.streaming = Some(StreamingTurn::new(Arc::new(AtomicBool::new(false))));
        sender
            .send(PendingEvent::TextDelta("Hel".to_owned()))
            .unwrap();
        sender
            .send(PendingEvent::TextDelta("lo".to_owned()))
            .unwrap();
        app.poll_response();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Hello▍"), "{text}");
        assert!(text.contains("Esc to cancel"), "{text}");
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::draw;
    use crate::Settings;
    use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn app_with_answer(text: String) -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.settings.background_animation = false;
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Assistant,
            text,
        });
        app
    }

    #[test]
    fn the_last_line_of_a_long_answer_is_visible_when_words_wrap() {
        // Three 30-character words wrap onto three screen lines, not the two that
        // dividing the line width by the screen width would suggest.
        let word = "abcdefghijklmnopqrstuvwxyz0123";
        let line = format!("{word} {word} {word}");
        let mut text = vec![line; 14];
        text.push("ENDMARK".to_owned());
        let app = app_with_answer(text.join(
            "
",
        ));
        let shown = screen(&app, 50, 24);
        assert!(
            shown.contains("ENDMARK"),
            "the answer's last line is cut off"
        );
    }

    #[test]
    fn a_streaming_answer_stays_pinned_to_its_newest_text() {
        let word = "abcdefghijklmnopqrstuvwxyz0123";
        let mut app = app_with_answer("hello".to_owned());
        let (sender, receiver) = std::sync::mpsc::channel();
        app.pending = Some(receiver);
        app.streaming = Some(crate::tui::state::StreamingTurn::new(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        for _ in 0..14 {
            sender
                .send(crate::agent::PendingEvent::TextDelta(format!(
                    "{word} {word} {word}
"
                )))
                .unwrap();
        }
        sender
            .send(crate::agent::PendingEvent::TextDelta(
                "NEWESTWORD".to_owned(),
            ))
            .unwrap();
        app.poll_response();
        assert!(screen(&app, 50, 24).contains("NEWESTWORD"));
    }
}
