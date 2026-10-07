use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Width of the large logo; terminals narrower than this get the compact one.
pub(super) const FULL_WIDTH: u16 = 70;
/// Width of the compact logo; terminals narrower than this get a single line.
pub(super) const COMPACT_WIDTH: u16 = 66;

const FULL_ROWS: usize = 6;
const COMPACT_ROWS: usize = 5;
const TAGLINE: &str = "temperature-conscious coding harness";

/// How many rows the logo takes on a terminal `width` columns wide.
pub(super) fn wordmark_height(width: u16) -> u16 {
    if width >= FULL_WIDTH {
        FULL_ROWS as u16
    } else if width >= COMPACT_WIDTH {
        COMPACT_ROWS as u16
    } else {
        1
    }
}

/// The logo for `width` columns; `elapsed` is seconds since launch (the bloom plays in the first).
pub(super) fn cool_code_wordmark(elapsed: f32, width: u16) -> Vec<Line<'static>> {
    if width >= FULL_WIDTH {
        full_wordmark(elapsed)
    } else if width >= COMPACT_WIDTH {
        compact_wordmark(elapsed.clamp(0.0, 1.0))
    } else {
        vec![Line::from(vec![
            Span::styled("◆ ", Style::default().fg(Color::Rgb(135, 226, 250))),
            Span::styled(
                "COOL",
                Style::default()
                    .fg(Color::Rgb(135, 226, 250))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " CODE",
                Style::default()
                    .fg(Color::Rgb(205, 213, 224))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ◆", Style::default().fg(Color::Rgb(135, 226, 250))),
        ])]
    }
}

fn full_glyph(character: char) -> [&'static str; FULL_ROWS] {
    match character {
        'C' => [
            " ██████╗",
            "██╔════╝",
            "██║     ",
            "██║     ",
            "╚██████╗",
            " ╚═════╝",
        ],
        'O' => [
            " ██████╗ ",
            "██╔═══██╗",
            "██║   ██║",
            "██║   ██║",
            "╚██████╔╝",
            " ╚═════╝ ",
        ],
        'L' => [
            "██╗     ",
            "██║     ",
            "██║     ",
            "██║     ",
            "███████╗",
            "╚══════╝",
        ],
        'D' => [
            "██████╗ ",
            "██╔══██╗",
            "██║  ██║",
            "██║  ██║",
            "██████╔╝",
            "╚═════╝ ",
        ],
        'E' => [
            "███████╗",
            "██╔════╝",
            "█████╗  ",
            "██╔══╝  ",
            "███████╗",
            "╚══════╝",
        ],
        _ => [""; FULL_ROWS],
    }
}

/// Silver for the second word, so the two halves read as warm and cold metal.
fn silver_gradient_color(position: f32) -> Color {
    let palette = [
        [150.0, 162.0, 178.0],
        [226.0, 232.0, 240.0],
        [170.0, 182.0, 198.0],
    ];
    let phase = position.rem_euclid(12.0) / 12.0 * palette.len() as f32;
    let left = phase.floor() as usize % palette.len();
    let right = (left + 1) % palette.len();
    let fraction = phase.fract();
    let blend = |channel: usize| {
        (palette[left][channel] * (1.0 - fraction) + palette[right][channel] * fraction).round()
            as u8
    };
    Color::Rgb(blend(0), blend(1), blend(2))
}

fn full_wordmark(elapsed: f32) -> Vec<Line<'static>> {
    let words = [['C', 'O', 'O', 'L'], ['C', 'O', 'D', 'E']];
    let bloom = elapsed.clamp(0.0, 1.0);
    let progress = 1.0 - (1.0 - bloom).powi(3);
    let wave_radius = progress * 46.0;
    // After the bloom, a soft glint travels across the letters every few seconds.
    let glint_at = ((elapsed - 1.2) * 0.2).rem_euclid(1.6) * f32::from(FULL_WIDTH)
        - 0.3 * f32::from(FULL_WIDTH);
    let glint_on = elapsed > 1.2;
    (0..FULL_ROWS)
        .map(|row| {
            let mut spans = Vec::new();
            let mut column = 0usize;
            for (word_index, letters) in words.iter().enumerate() {
                for character in letters {
                    for symbol in full_glyph(*character)[row].chars() {
                        if symbol == ' ' {
                            spans.push(Span::raw(" "));
                            column += 1;
                            continue;
                        }
                        let base = if word_index == 0 {
                            ice_gradient_color(column as f32 * 0.4)
                        } else {
                            silver_gradient_color(column as f32 * 0.3)
                        };
                        let solid = symbol == '█';
                        let mut color = if solid {
                            base
                        } else {
                            // The strokes that draw the shadow sit back in the dark.
                            logo_blend_color(base, Color::Rgb(12, 18, 28), 0.58)
                        };
                        let horizontal = column as f32 - f32::from(FULL_WIDTH) / 2.0;
                        let vertical = (row as f32 - 2.5) * 2.0;
                        let distance = ((horizontal * horizontal + vertical * vertical).sqrt()
                            - wave_radius)
                            .abs();
                        if bloom < 1.0 && distance < 5.0 {
                            color = logo_blend_color(
                                color,
                                Color::Rgb(232, 251, 255),
                                (1.0 - distance / 5.0) * 0.94,
                            );
                        }
                        if glint_on {
                            let gap = column as f32 - glint_at;
                            color = logo_blend_color(
                                color,
                                Color::Rgb(240, 252, 255),
                                0.5 * (-(gap * gap) / 24.0).exp(),
                            );
                        }
                        let glyph = if solid
                            && bloom < 0.88
                            && distance < 2.5
                            && (column + row).is_multiple_of(2)
                        {
                            "✦".to_owned()
                        } else {
                            symbol.to_string()
                        };
                        spans.push(Span::styled(
                            glyph,
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ));
                        column += 1;
                    }
                }
                if word_index == 0 {
                    spans.push(Span::raw("   "));
                    column += 3;
                }
            }
            Line::from(spans)
        })
        .collect()
}

fn compact_wordmark(elapsed: f32) -> Vec<Line<'static>> {
    fn glyph(character: char) -> [&'static str; 5] {
        match character {
            'C' => ["0111110", "1100011", "1100000", "1100011", "0111110"],
            'O' => ["0111110", "1100011", "1100011", "1100011", "0111110"],
            'L' => ["1100000", "1100000", "1100000", "1100011", "1111111"],
            'D' => ["1111100", "1100110", "1100011", "1100110", "1111100"],
            'E' => ["1111111", "1100000", "1111100", "1100000", "1111111"],
            _ => ["0000000"; 5],
        }
    }
    let cool = ['C', 'O', 'O', 'L'];
    let code = ['C', 'O', 'D', 'E'];
    let elapsed = elapsed.clamp(0.0, 1.0);
    let progress = 1.0 - (1.0 - elapsed).powi(3);
    let wave_radius = progress * 23.0;
    (0..COMPACT_ROWS)
        .map(|row| {
            let mut spans = Vec::new();
            let mut column = 0usize;
            for (word_index, letters) in [&cool[..], &code[..]].iter().enumerate() {
                for (letter_index, character) in letters.iter().enumerate() {
                    for lit in glyph(*character)[row].chars() {
                        if lit == '1' {
                            let distance = if word_index == 0 {
                                let horizontal = column as f32 - 16.0;
                                let vertical = row as f32 - 2.0;
                                let radius = (horizontal * horizontal + vertical * vertical).sqrt();
                                Some((radius - wave_radius).abs())
                            } else {
                                None
                            };
                            let color = if word_index == 0 {
                                let distance = distance.unwrap_or_default();
                                let base = ice_gradient_color(column as f32 * 0.62);
                                if elapsed < 1.0 && distance < 4.5 {
                                    logo_blend_color(
                                        base,
                                        Color::Rgb(232, 251, 255),
                                        (1.0 - distance / 4.5) * 0.94,
                                    )
                                } else {
                                    base
                                }
                            } else {
                                Color::Rgb(139, 146, 156)
                            };
                            spans.push(Span::styled(
                                if word_index == 0
                                    && elapsed < 0.88
                                    && distance.is_some_and(|distance| distance < 2.3)
                                    && (column + row).is_multiple_of(2)
                                {
                                    "✦"
                                } else {
                                    "█"
                                },
                                Style::default().fg(color).add_modifier(Modifier::BOLD),
                            ));
                        } else {
                            spans.push(Span::raw(" "));
                        }
                        column += 1;
                    }
                    if letter_index + 1 < letters.len() {
                        spans.push(Span::raw(" "));
                    }
                }
                if word_index == 0 {
                    spans.extend([Span::raw("  "), Span::raw("  ")]);
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// The two lines under the logo: the tagline, with a glint sweeping through it, and a version row.
pub(super) fn tagline_lines(elapsed: f32) -> Vec<Line<'static>> {
    let fade = (elapsed * 1.5).clamp(0.0, 1.0);
    let length = TAGLINE.chars().count() as f32;
    let sweep = ((elapsed * 0.22).rem_euclid(1.6) - 0.3) * length;
    let tagline = TAGLINE
        .chars()
        .enumerate()
        .map(|(index, character)| {
            let gap = index as f32 - sweep;
            let base = logo_blend_color(Color::Rgb(8, 10, 14), Color::Rgb(128, 158, 184), fade);
            let color = logo_blend_color(
                base,
                Color::Rgb(240, 252, 255),
                0.85 * fade * (-(gap * gap) / 10.0).exp(),
            );
            Span::styled(character.to_string(), Style::default().fg(color))
        })
        .collect::<Vec<_>>();
    let rule = Style::default().fg(Color::Rgb(58, 80, 100));
    let ice = Style::default().fg(Color::Rgb(135, 226, 250));
    let detail = Style::default().fg(Color::Rgb(108, 128, 148));
    let version = Line::from(vec![
        Span::styled("──────  ", rule),
        Span::styled("◆", ice),
        Span::styled(format!("  Rust · v{}  ", env!("CARGO_PKG_VERSION")), detail),
        Span::styled("◆", ice),
        Span::styled("  ──────", rule),
    ]);
    vec![Line::from(tagline), version]
}

pub(super) fn logo_blend_color(left: Color, right: Color, amount: f32) -> Color {
    let (Color::Rgb(lr, lg, lb), Color::Rgb(rr, rg, rb)) = (left, right) else {
        return right;
    };
    let blend = |a: u8, b: u8| (a as f32 * (1.0 - amount) + b as f32 * amount).round() as u8;
    Color::Rgb(blend(lr, rr), blend(lg, rg), blend(lb, rb))
}

pub(super) fn ice_gradient_color(position: f32) -> Color {
    let palette = [
        [83.0, 197.0, 237.0],
        [135.0, 226.0, 250.0],
        [215.0, 249.0, 255.0],
        [130.0, 190.0, 246.0],
    ];
    let phase = position.rem_euclid(16.0) / 16.0 * palette.len() as f32;
    let left = phase.floor() as usize % palette.len();
    let right = (left + 1) % palette.len();
    let fraction = phase.fract();
    let blend = |channel: usize| {
        (palette[left][channel] * (1.0 - fraction) + palette[right][channel] * fraction).round()
            as u8
    };
    Color::Rgb(blend(0), blend(1), blend(2))
}

#[cfg(test)]
mod tests {
    use super::{COMPACT_WIDTH, FULL_WIDTH, cool_code_wordmark, tagline_lines, wordmark_height};
    use ratatui::style::Color;
    use ratatui::text::Line;

    fn width_of(line: &Line<'_>) -> usize {
        line.spans
            .iter()
            .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
            .sum()
    }

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn brightness(color: Option<Color>) -> u32 {
        match color {
            Some(Color::Rgb(r, g, b)) => u32::from(r) + u32::from(g) + u32::from(b),
            other => panic!("expected RGB, got {other:?}"),
        }
    }

    #[test]
    fn the_logo_size_follows_the_terminal_width() {
        assert_eq!(wordmark_height(FULL_WIDTH), 6);
        assert_eq!(wordmark_height(FULL_WIDTH - 1), 5);
        assert_eq!(wordmark_height(COMPACT_WIDTH), 5);
        assert_eq!(wordmark_height(COMPACT_WIDTH - 1), 1);
        assert_eq!(wordmark_height(0), 1);
        for width in [0, 20, COMPACT_WIDTH, FULL_WIDTH, 200] {
            assert_eq!(
                cool_code_wordmark(2.0, width).len(),
                usize::from(wordmark_height(width)),
                "width {width}"
            );
        }
    }

    #[test]
    fn the_large_logo_is_six_even_rows_that_fit_its_width() {
        let lines = cool_code_wordmark(2.0, 120);
        assert_eq!(lines.len(), 6);
        for line in &lines {
            assert_eq!(
                width_of(line),
                usize::from(FULL_WIDTH),
                "{}",
                text_of(&lines)
            );
        }
        let drawing = text_of(&lines);
        assert!(drawing.contains('█') && drawing.contains('╗') && drawing.contains('╚'));
    }

    #[test]
    fn the_shadow_strokes_sit_darker_than_the_solid_letters() {
        let lines = cool_code_wordmark(2.0, 120);
        let average = |wanted: fn(&str) -> bool| {
            let values = lines
                .iter()
                .flat_map(|line| &line.spans)
                .filter(|span| wanted(span.content.as_ref()))
                .map(|span| brightness(span.style.fg))
                .collect::<Vec<_>>();
            assert!(!values.is_empty());
            values.iter().sum::<u32>() as f32 / values.len() as f32
        };
        let solid = average(|symbol| symbol == "█");
        let shadow = average(|symbol| matches!(symbol, "╗" | "╝" | "╔" | "╚" | "═" | "║"));
        assert!(shadow < solid * 0.75, "shadow {shadow} vs solid {solid}");
    }

    #[test]
    fn the_two_words_have_their_own_metals() {
        let lines = cool_code_wordmark(2.0, 120);
        let colors = |from: usize, to: usize| {
            lines[1]
                .spans
                .iter()
                .skip(from)
                .take(to - from)
                .filter(|span| span.content.as_ref() == "█")
                .map(|span| span.style.fg)
                .collect::<Vec<_>>()
        };
        let cool = colors(0, 30);
        let code = colors(40, 70);
        let bluish = |c: &Option<Color>| matches!(c, Some(Color::Rgb(r, _, b)) if *b > *r + 40);
        let neutral =
            |c: &Option<Color>| matches!(c, Some(Color::Rgb(r, _, b)) if b.abs_diff(*r) < 40);
        assert!(cool.iter().all(bluish), "COOL is ice-blue");
        assert!(code.iter().all(neutral), "CODE is silver");
    }

    #[test]
    fn the_bloom_plays_once_at_launch_on_every_size() {
        for width in [FULL_WIDTH, COMPACT_WIDTH] {
            let during = text_of(&cool_code_wordmark(0.22, width));
            assert!(during.contains('✦'), "width {width}: bloom sparkles");
            let after = text_of(&cool_code_wordmark(3.0, width));
            assert!(!after.contains('✦'), "width {width}: sparkles are gone");
        }
    }

    #[test]
    fn a_glint_crosses_the_large_logo_after_the_bloom() {
        let colors = |t: f32| {
            cool_code_wordmark(t, 120)
                .iter()
                .flat_map(|line| line.spans.iter().map(|span| span.style.fg))
                .collect::<Vec<_>>()
        };
        assert_ne!(colors(2.0), colors(4.0));
        assert_eq!(
            text_of(&cool_code_wordmark(2.0, 120)),
            text_of(&cool_code_wordmark(4.0, 120)),
            "only colors move"
        );
    }

    #[test]
    fn a_narrow_terminal_gets_a_one_line_name() {
        let lines = cool_code_wordmark(2.0, 30);
        assert_eq!(lines.len(), 1);
        assert!(text_of(&lines).contains("COOL CODE"));
        assert!(width_of(&lines[0]) <= 30);
    }

    #[test]
    fn the_tagline_row_names_the_harness_and_its_version() {
        let lines = tagline_lines(5.0);
        assert_eq!(lines.len(), 2);
        let shown = text_of(&lines);
        assert!(
            shown.contains("temperature-conscious coding harness"),
            "{shown}"
        );
        assert!(
            shown.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))),
            "{shown}"
        );
    }

    #[test]
    fn the_tagline_fades_in_and_shimmers() {
        let first = |t: f32| tagline_lines(t)[0].spans[0].style.fg;
        assert!(brightness(first(0.0)) < brightness(first(1.0)));
        let colors = |t: f32| {
            tagline_lines(t)[0]
                .spans
                .iter()
                .map(|span| span.style.fg)
                .collect::<Vec<_>>()
        };
        assert_ne!(colors(2.0), colors(4.5));
    }
}
