use crate::PulseMode;
use crate::tui::render::centered_rect;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::time::{Duration, Instant};

pub(super) const BASE_TEXT: Color = Color::Rgb(226, 230, 235);
pub(super) const PULSE_DURATION: Duration = Duration::from_millis(500);

// Freshly arrived text glows toward this icy white before settling to BASE_TEXT.
const GLOW: (f32, f32, f32) = (190.0, 240.0, 255.0);
const BASE: (f32, f32, f32) = (226.0, 230.0, 235.0);

/// 1.0 for text that just arrived, fading linearly to 0.0 after `PULSE_DURATION`.
pub(super) fn pulse_strength(age: Duration) -> f32 {
    (1.0 - age.as_secs_f32() / PULSE_DURATION.as_secs_f32()).clamp(0.0, 1.0)
}

fn color_for(strength: f32) -> Color {
    if strength <= 0.0 {
        return BASE_TEXT;
    }
    let mix = |base: f32, glow: f32| (base + (glow - base) * strength).round() as u8;
    Color::Rgb(
        mix(BASE.0, GLOW.0),
        mix(BASE.1, GLOW.1),
        mix(BASE.2, GLOW.2),
    )
}

/// Splits streamed text into styled spans, brightening text that arrived recently.
pub(super) fn pulse_spans(
    text: &str,
    arrivals: &[(usize, Instant)],
    mode: PulseMode,
    now: Instant,
) -> Vec<Span<'static>> {
    if mode == PulseMode::Off || text.is_empty() {
        return vec![Span::styled(
            text.to_owned(),
            Style::default().fg(BASE_TEXT),
        )];
    }
    let arrived_at = |byte: usize| {
        arrivals
            .iter()
            .rev()
            .find(|(offset, _)| *offset <= byte)
            .map(|(_, at)| *at)
    };
    let age = |byte: usize| {
        arrived_at(byte).map_or(PULSE_DURATION, |at| now.saturating_duration_since(at))
    };
    let chars = text.char_indices().collect::<Vec<_>>();
    let mut strengths = vec![0.0_f32; chars.len()];
    match mode {
        PulseMode::Characters => {
            for (index, (byte, _)) in chars.iter().enumerate() {
                strengths[index] = pulse_strength(age(*byte));
            }
        }
        _ => {
            // A word pulses once it is complete, timed by the arrival of its last character.
            let mut start = None;
            for index in 0..=chars.len() {
                let is_word_char = chars.get(index).is_some_and(|(_, c)| !c.is_whitespace());
                match (start, is_word_char) {
                    (None, true) => start = Some(index),
                    (Some(first), false) if index < chars.len() => {
                        let strength = pulse_strength(age(chars[index - 1].0));
                        strengths[first..index].fill(strength);
                        start = None;
                    }
                    _ => {}
                }
            }
        }
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current = String::new();
    let mut current_color = None;
    for ((_, character), strength) in chars.iter().zip(strengths) {
        let color = color_for((strength * 10.0).round() / 10.0);
        if current_color.is_some_and(|previous| previous != color) {
            spans.push(Span::styled(
                std::mem::take(&mut current),
                Style::default().fg(current_color.unwrap()),
            ));
        }
        current_color = Some(color);
        current.push(*character);
    }
    if let Some(color) = current_color {
        spans.push(Span::styled(current, Style::default().fg(color)));
    }
    spans
}

pub(super) fn draw_motion_prompt(frame: &mut ratatui::Frame<'_>, area: Rect, choice: usize) {
    let accent = Color::Rgb(98, 213, 244);
    let popup = centered_rect(64, 36, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Motion ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(25, 32, 38)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let option = |label: &'static str, selected: bool| {
        if selected {
            Span::styled(
                format!("[ {label} ]"),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(label, Style::default().fg(Color::Gray))
        }
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Do you prefer reduced motion?",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Text pulses and the background animation can be turned off. You can change this later in Settings → General.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(vec![
            option("Keep animations", choice == 0),
            Span::raw("     "),
            option("Reduce motion", choice == 1),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "←/→ choose   Enter confirm",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::{BASE_TEXT, pulse_spans, pulse_strength};
    use crate::PulseMode;
    use std::time::{Duration, Instant};

    #[test]
    fn pulse_fades_to_zero_after_half_a_second() {
        assert_eq!(pulse_strength(Duration::ZERO), 1.0);
        let middle = pulse_strength(Duration::from_millis(250));
        assert!((middle - 0.5).abs() < 0.01, "{middle}");
        assert_eq!(pulse_strength(Duration::from_millis(500)), 0.0);
        assert_eq!(pulse_strength(Duration::from_secs(3)), 0.0);
    }

    fn styled(text: &str, mode: PulseMode) -> Vec<(String, bool)> {
        let now = Instant::now();
        pulse_spans(text, &[(0, now)], mode, now)
            .into_iter()
            .map(|span| (span.content.to_string(), span.style.fg != Some(BASE_TEXT)))
            .collect()
    }

    #[test]
    fn words_mode_pulses_only_completed_words() {
        let spans = styled("hello wor", PulseMode::Words);
        let joined = spans
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<String>();
        assert_eq!(joined, "hello wor");
        assert!(
            spans
                .iter()
                .any(|(text, lit)| text.contains("hello") && *lit)
        );
        assert!(
            spans
                .iter()
                .any(|(text, lit)| text.contains("wor") && !*lit)
        );
    }

    #[test]
    fn characters_mode_pulses_everything_fresh() {
        let spans = styled("hello wor", PulseMode::Characters);
        assert!(
            spans
                .iter()
                .all(|(text, lit)| *lit || text.trim().is_empty())
        );
    }

    #[test]
    fn pulse_off_draws_plain_text() {
        let spans = styled("hello wor", PulseMode::Off);
        assert_eq!(spans, vec![("hello wor".to_owned(), false)]);
    }

    #[test]
    fn old_text_is_drawn_plain() {
        let long_ago = Instant::now() - Duration::from_secs(5);
        let spans = pulse_spans("done ", &[(0, long_ago)], PulseMode::Words, Instant::now());
        assert!(spans.iter().all(|span| span.style.fg == Some(BASE_TEXT)));
    }
}
