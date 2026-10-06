use crate::Effort;
use crate::tui::centered_rect;
use crate::tui::state::{App, LEVELS};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(super) fn draw_effort_picker(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    animation_tick: usize,
) {
    let popup = centered_rect(96, 54, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Select effort ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(98, 213, 244)))
        .style(Style::default().bg(Color::Rgb(29, 30, 32)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let selected_effort = LEVELS[app.picker_index];
    let columns = (0..LEVELS.len())
        .map(|index| {
            let left = inner.x + inner.width * index as u16 / LEVELS.len() as u16;
            let right = inner.x + inner.width * (index as u16 + 1) / LEVELS.len() as u16;
            Rect::new(left, inner.y, right.saturating_sub(left), inner.height)
        })
        .collect::<Vec<_>>();

    let names = ["Low", "Medium", "High", "XHigh", "Max", "Super", "Extreme"];
    let mappings = [
        "low", "medium", "high", "xhigh", "max", "xhigh+wf", "max+wf",
    ];
    for index in 0..LEVELS.len() {
        let column = columns[index];
        let selected = index == app.picker_index;
        let label = if selected {
            let mut spans = vec![Span::styled("›", Style::default().fg(Color::White))];
            spans.extend(gradient_name(LEVELS[index], true, animation_tick));
            Line::from(spans)
        } else {
            Line::from(Span::styled(names[index], Style::default().fg(Color::Gray)))
        };
        frame.render_widget(
            Paragraph::new(label).alignment(Alignment::Center),
            Rect::new(column.x, column.y + 1, column.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                mappings[index],
                Style::default().fg(if selected {
                    effort_rgb(index, animation_tick, 0.8)
                } else {
                    Color::DarkGray
                }),
            ))
            .alignment(Alignment::Center),
            Rect::new(column.x, column.y + 2, column.width, 1),
        );
        frame.render_widget(
            Paragraph::new(bar_segment(
                index,
                app.picker_index,
                column.width,
                animation_tick,
            ))
            .alignment(Alignment::Center),
            Rect::new(column.x, column.y + 4, column.width, 1),
        );
    }

    let description = Paragraph::new(selected_effort.description())
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(description, Rect::new(inner.x, inner.y + 6, inner.width, 2));
    frame.render_widget(
        Paragraph::new("←/→ move   Enter select   Esc cancel")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
    );
}

pub(super) fn effort_name(effort: Effort) -> String {
    format!("{effort:?}").to_ascii_lowercase()
}

pub(super) fn effort_style(effort: Effort, selected: bool) -> Style {
    let color = match effort {
        Effort::Max => Color::Magenta,
        Effort::XHigh => Color::LightBlue,
        Effort::Super => Color::Yellow,
        Effort::Extreme => Color::Red,
        _ => Color::White,
    };
    let style = Style::default().fg(color);
    if selected {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

pub(super) fn bar_segment(
    index: usize,
    selected_index: usize,
    width: u16,
    animation_tick: usize,
) -> Line<'static> {
    let content_width = width.saturating_sub(2).max(1) as usize;
    let mut spans = Vec::with_capacity(content_width);
    if index == selected_index {
        for character_index in 0..content_width {
            let height = selected_bar_height(LEVELS[index], character_index, animation_tick);
            let edge_fade = if character_index == 0 || character_index + 1 == content_width {
                0.96
            } else {
                1.0
            };
            let color = selected_bar_color(LEVELS[index], character_index, height, edge_fade);
            spans.push(Span::styled(
                height_glyph(height),
                Style::default().fg(color),
            ));
        }
    } else {
        let star_color = effort_rgb(index, animation_tick, 0.25);
        let selected_color = if matches!(
            LEVELS[selected_index],
            Effort::Max | Effort::XHigh | Effort::Super | Effort::Extreme
        ) {
            animated_effort_color(LEVELS[selected_index], animation_tick)
        } else {
            effort_rgb(selected_index, animation_tick, 1.0)
        };
        let star_position = (animation_tick + index * 3) % content_width;
        let boundary_position = if index + 1 == selected_index {
            Some(content_width - 1)
        } else if index == selected_index + 1 {
            Some(0)
        } else {
            None
        };
        for character_index in 0..content_width {
            let character =
                if character_index == star_position || boundary_position == Some(character_index) {
                    "✦"
                } else if character_index % 3 == 0 {
                    "·"
                } else {
                    " "
                };
            let is_edge = character_index < 2 || character_index + 2 >= content_width;
            let edge_fade = if is_edge { 0.96 } else { 1.0 };
            let is_star = character != " ";
            let brightness =
                if character_index == star_position || boundary_position == Some(character_index) {
                    0.34 * edge_fade
                } else if is_star {
                    0.22 * edge_fade
                } else {
                    0.0
                };
            let mut color = scale_color(star_color, brightness / 0.25);
            let spill = if index + 1 == selected_index {
                if character_index + 1 == content_width {
                    0.42
                } else if character_index + 2 == content_width {
                    0.22
                } else {
                    0.0
                }
            } else if index == selected_index + 1 {
                if character_index == 0 {
                    0.42
                } else if character_index == 1 {
                    0.22
                } else {
                    0.0
                }
            } else {
                0.0
            };
            if spill > 0.0 {
                color = blend_color(color, selected_color, spill);
            }
            spans.push(Span::styled(character, Style::default().fg(color)));
        }
    }
    Line::from(spans)
}

pub(super) fn height_glyph(height: usize) -> &'static str {
    ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][height.saturating_sub(1).min(7)]
}

pub(super) fn selected_bar_height(effort: Effort, column: usize, animation_tick: usize) -> usize {
    match effort {
        Effort::Low | Effort::Medium => 1,
        Effort::High => 2,
        Effort::XHigh => [4, 4, 3, 3, 2, 2, 1][column % 7],
        Effort::Max => {
            let base = [8, 7, 6, 5, 4, 3, 2][column % 7];
            if base < 8 && (column * 3 + animation_tick * 2) % 11 == 0 {
                base + 1
            } else {
                base
            }
        }
        Effort::Super => {
            if (column + animation_tick) % 2 == 0 {
                8
            } else {
                1
            }
        }
        Effort::Extreme => {
            let flicker = (column * 31 + animation_tick * 17 + column * animation_tick * 13) % 7;
            2 + flicker
        }
    }
}

pub(super) fn selected_bar_color(
    effort: Effort,
    position: usize,
    height: usize,
    edge_fade: f32,
) -> Color {
    let color = match effort {
        Effort::Extreme if height >= 7 => Color::Rgb(255, 221, 112),
        Effort::Extreme if height >= 5 => Color::Rgb(255, 143, 65),
        Effort::Extreme if height >= 3 => Color::Rgb(251, 81, 59),
        Effort::Extreme => Color::Rgb(177, 43, 78),
        Effort::Max | Effort::XHigh | Effort::Super => animated_effort_color(effort, position),
        _ => effort_rgb(
            LEVELS
                .iter()
                .position(|level| *level == effort)
                .unwrap_or(0),
            position,
            1.0,
        ),
    };
    scale_color(color, edge_fade)
}

pub(super) fn effort_rgb(index: usize, _animation_tick: usize, brightness: f32) -> Color {
    let color = match LEVELS[index] {
        Effort::Low => (137, 148, 164),
        Effort::Medium => (94, 148, 235),
        Effort::High => (65, 197, 214),
        Effort::Max => (190, 105, 210),
        Effort::XHigh => (155, 125, 240),
        Effort::Super => (241, 184, 63),
        Effort::Extreme => (229, 66, 74),
    };
    scale_rgb(color, brightness)
}

pub(super) fn animated_effort_color(effort: Effort, phase: usize) -> Color {
    let index = match effort {
        Effort::Max => phase % 7,
        Effort::XHigh => phase % 5,
        Effort::Super => phase % 4,
        Effort::Extreme => phase % 4,
        _ => 0,
    };
    let colors: &[(u8, u8, u8)] = match effort {
        Effort::Max => &[
            (255, 90, 90),
            (255, 166, 70),
            (248, 224, 84),
            (100, 220, 130),
            (84, 198, 236),
            (127, 130, 255),
            (220, 115, 238),
        ],
        Effort::XHigh => &[
            (174, 203, 255),
            (140, 170, 255),
            (153, 132, 255),
            (188, 145, 255),
            (154, 192, 255),
        ],
        Effort::Super => &[
            (255, 231, 130),
            (255, 195, 64),
            (240, 157, 38),
            (255, 216, 90),
        ],
        Effort::Extreme => &[
            (255, 151, 151),
            (249, 75, 79),
            (204, 35, 56),
            (255, 103, 80),
        ],
        _ => &[(255, 255, 255)],
    };
    let (red, green, blue) = colors[index];
    Color::Rgb(red, green, blue)
}

pub(super) fn scale_rgb(color: (u8, u8, u8), amount: f32) -> Color {
    Color::Rgb(
        (color.0 as f32 * amount).min(255.0) as u8,
        (color.1 as f32 * amount).min(255.0) as u8,
        (color.2 as f32 * amount).min(255.0) as u8,
    )
}

pub(super) fn scale_color(color: Color, amount: f32) -> Color {
    match color {
        Color::Rgb(red, green, blue) => scale_rgb((red, green, blue), amount),
        other => other,
    }
}

pub(super) fn blend_color(from: Color, to: Color, amount: f32) -> Color {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) = (from, to) else {
        return from;
    };
    let blend = |left: u8, right: u8| {
        (left as f32 + (right as f32 - left as f32) * amount).clamp(0.0, 255.0) as u8
    };
    Color::Rgb(blend(fr, tr), blend(fg, tg), blend(fb, tb))
}

pub(super) fn gradient_name(
    effort: Effort,
    selected: bool,
    animation_tick: usize,
) -> Vec<Span<'static>> {
    let name = effort_label(effort).to_owned();
    if !selected
        || !matches!(
            effort,
            Effort::Max | Effort::XHigh | Effort::Super | Effort::Extreme
        )
    {
        return vec![Span::styled(name, effort_style(effort, selected))];
    }
    let colors: &[Color] = match effort {
        Effort::Max => &[
            Color::Red,
            Color::Rgb(255, 128, 0),
            Color::Yellow,
            Color::Green,
            Color::Cyan,
            Color::Blue,
            Color::Magenta,
        ],
        Effort::XHigh => &[
            Color::Rgb(174, 203, 255),
            Color::Rgb(140, 170, 255),
            Color::Rgb(153, 132, 255),
            Color::Rgb(188, 145, 255),
            Color::Rgb(154, 192, 255),
        ],
        Effort::Super => &[
            Color::Rgb(255, 255, 150),
            Color::Yellow,
            Color::Rgb(255, 190, 0),
            Color::Rgb(230, 145, 0),
            Color::Rgb(255, 225, 100),
        ],
        Effort::Extreme => &[
            Color::Rgb(255, 180, 180),
            Color::LightRed,
            Color::Red,
            Color::Rgb(210, 20, 30),
            Color::Rgb(145, 0, 20),
            Color::Rgb(255, 90, 75),
            Color::Red,
        ],
        _ => unreachable!(),
    };
    let spans = name
        .chars()
        .enumerate()
        .map(|(index, character)| {
            Span::styled(
                character.to_string(),
                Style::default()
                    .fg(colors[(index + animation_tick) % colors.len()])
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    spans
}

pub(super) fn effort_label(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "Low",
        Effort::Medium => "Medium",
        Effort::High => "High",
        Effort::Max => "Max",
        Effort::XHigh => "XHigh",
        Effort::Super => "Super",
        Effort::Extreme => "Extreme",
    }
}

#[cfg(test)]
mod tests {
    use super::{animated_effort_color, height_glyph, selected_bar_height};
    use crate::tui::draw;
    use crate::tui::state::App;
    use crate::{Effort, Settings};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn effort_picker_renders_horizontal_levels_and_xhigh_mapping() {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.picker = true;

        terminal
            .draw(|frame| draw(frame, &app, 0))
            .expect("draw picker");

        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for expected in [
            "Low", "Medium", "High", "XHigh", "Max", "Super", "Extreme", "xhigh+wf",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected} in picker:\n{rendered}"
            );
        }
        let ordered = ["Low", "Medium", "High", "XHigh", "Max", "Super", "Extreme"]
            .map(|label| rendered.find(label).expect("effort label"));
        assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn advanced_effort_highlights_change_color_over_time() {
        for effort in [Effort::Max, Effort::XHigh, Effort::Super, Effort::Extreme] {
            assert_ne!(
                animated_effort_color(effort, 0),
                animated_effort_color(effort, 1),
                "{effort:?} highlight should animate"
            );
        }
    }

    #[test]
    fn effort_bar_height_profiles_match_the_selected_tier() {
        assert_eq!(selected_bar_height(Effort::Max, 0, 0), 8);
        assert_eq!(selected_bar_height(Effort::XHigh, 0, 0), 4);
        assert_eq!(height_glyph(8), "█");
        assert_eq!(height_glyph(1), "▁");
        assert_ne!(
            selected_bar_height(Effort::Super, 0, 0),
            selected_bar_height(Effort::Super, 1, 0)
        );
        assert_ne!(
            selected_bar_height(Effort::Extreme, 0, 0),
            selected_bar_height(Effort::Extreme, 0, 1)
        );
    }
}
