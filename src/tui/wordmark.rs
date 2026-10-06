use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub(super) fn cool_code_wordmark(elapsed: f32) -> Vec<Line<'static>> {
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
    (0..5)
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
                                    && (column + row) % 2 == 0
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
    use super::cool_code_wordmark;

    #[test]
    fn welcome_wordmark_is_wide_and_has_a_visible_bloom_phase() {
        let lines = cool_code_wordmark(0.22);
        let width = lines[0]
            .spans
            .iter()
            .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>();
        assert!(width >= 64, "wordmark should have a wider visual footprint");
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.content == "✦")
        );
        assert_eq!(lines.len(), 5);
    }
}
