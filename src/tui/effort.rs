use crate::Effort;
use crate::tui::state::{App, LEVELS};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

const GLYPHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const NAMES: [&str; 7] = ["Low", "Medium", "High", "XHigh", "Max", "Super", "Extreme"];
const MAPPINGS: [&str; 7] = [
    "low", "medium", "high", "xhigh", "max", "xhigh+wf", "max+wf",
];
// Rows used by everything except the bar: padding, labels, mappings, gaps, description, footer.
const CHROME_ROWS: u16 = 9;

pub(super) fn bar_rows(inner_height: u16) -> u16 {
    inner_height.saturating_sub(CHROME_ROWS).clamp(1, 4)
}

fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65_535.0
}

fn smooth_noise(x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let ease = |v: f32| v * v * (3.0 - 2.0 * v);
    let (sx, sy) = (ease(fx), ease(fy));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let top = hash(ix, iy) + (hash(ix + 1, iy) - hash(ix, iy)) * sx;
    let bottom = hash(ix, iy + 1) + (hash(ix + 1, iy + 1) - hash(ix, iy + 1)) * sx;
    top + (bottom - top) * sy
}

fn fbm(x: f32, y: f32) -> f32 {
    (smooth_noise(x, y) + 0.5 * smooth_noise(x * 2.1 + 17.0, y * 2.1 + 5.0)) / 1.5
}

/// Fraction of the bar's height that is lit for `effort` at horizontal position `x` (0..=1) and time `t` in seconds.
pub(super) fn effort_level(effort: Effort, x: f32, t: f32) -> f32 {
    let level = match effort {
        Effort::Low => 0.18,
        Effort::Medium => 0.32,
        Effort::High => 0.48,
        // Aurora: two curtains drifting at different speeds.
        Effort::XHigh => 0.55 + 0.2 * (x * 5.0 + t * 0.9).sin() + 0.15 * (x * 11.0 - t * 1.7).sin(),
        // Equalizer: every column bounces on its own smoothed noise.
        Effort::Max => 0.25 + 0.75 * smooth_noise(x * 14.0, t * 2.2),
        Effort::Super => 0.5 + 0.35 * (std::f32::consts::TAU * (x * 2.0 - t * 0.6)).sin(),
        Effort::Extreme => 0.5 + 0.84 * (smooth_noise(x * 7.0, t * 2.0) - 0.5),
    };
    level.clamp(0.0, 1.0)
}

fn hsv(hue: f32, saturation: f32, value: f32) -> Color {
    let h = hue.rem_euclid(1.0) * 6.0;
    let c = value * saturation;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = value - c;
    Color::Rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub(super) const WHITE_HOT: Color = Color::Rgb(255, 246, 214);
const PEAK_FALL_PER_SECOND: f32 = 0.35;

/// Height of the Max equalizer's floating peak cap: the recent maximum, falling slowly.
pub(super) fn max_peak(x: f32, t: f32) -> f32 {
    (0..12)
        .map(|step| {
            let age = step as f32 * 0.1;
            effort_level(Effort::Max, x, t - age) - age * PEAK_FALL_PER_SECOND
        })
        .fold(0.0_f32, f32::max)
        .clamp(0.0, 1.0)
}

fn wave(phase: f32) -> f32 {
    0.5 + 0.5 * phase.sin()
}

/// Color of a lit cell; `height` is the cell's vertical position within the bar (0 bottom, 1 top).
fn lit_color(effort: Effort, x: f32, height: f32, t: f32) -> Color {
    match effort {
        Effort::XHigh => {
            let curtain = blend_color(
                Color::Rgb(110, 120, 255),
                Color::Rgb(190, 130, 255),
                wave(x * 4.0 + t * 0.7),
            );
            let crest = (height - 0.55).max(0.0) * 1.6 * wave(x * 9.0 - t * 1.3);
            blend_color(curtain, Color::Rgb(120, 240, 220), crest.min(0.7))
        }
        Effort::Max => hsv(x * 0.85 + t * 0.12, 0.65, 0.75 + 0.25 * height),
        Effort::Super => blend_color(
            Color::Rgb(235, 150, 35),
            Color::Rgb(255, 235, 140),
            wave(std::f32::consts::TAU * (x * 2.0 - t * 0.6)),
        ),
        Effort::Extreme => fire_color(1.0 - height),
        Effort::Low => scale_color(effort_rgb(0, 0, 1.0), 0.9 + 0.12 * wave(t * 1.3)),
        Effort::Medium => glint(effort_rgb(1, 0, 1.0), x, t, 0.35),
        Effort::High => glint(
            scale_color(
                effort_rgb(2, 0, 1.0),
                0.95 + 0.08 * wave(x * 10.0 - t * 2.0),
            ),
            x,
            t,
            0.3,
        ),
    }
}

/// A soft highlight sweeping left to right every few seconds.
fn glint(base: Color, x: f32, t: f32, strength: f32) -> Color {
    let position = (t * 0.35).fract() * 1.4 - 0.2;
    let distance = x - position;
    let amount = strength * (-(distance * distance) / 0.01).exp();
    blend_color(base, Color::Rgb(255, 255, 255), amount)
}

/// `heat` runs from 0 (flame tip) to 1 (white-hot base).
fn fire_color(heat: f32) -> Color {
    let stops = [
        (0.0, Color::Rgb(120, 20, 40)),
        (0.3, Color::Rgb(220, 50, 40)),
        (0.55, Color::Rgb(255, 120, 40)),
        (0.78, Color::Rgb(255, 205, 90)),
        (1.0, WHITE_HOT),
    ];
    let heat = heat.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        let ((from_at, from), (to_at, to)) = (pair[0], pair[1]);
        if heat <= to_at {
            return blend_color(from, to, (heat - from_at) / (to_at - from_at));
        }
    }
    WHITE_HOT
}

/// Rising flame surface: the column's flame height plus tongues of noise scrolling upward.
fn flame_surface(x: f32, y: f32, t: f32) -> f32 {
    effort_level(Effort::Extreme, x, t) + 0.35 * (fbm(x * 5.0, y * 3.0 - t * 2.4) - 0.5)
}

fn ember_at(column: usize, row_from_bottom: usize, rows: usize, t: f32) -> Option<&'static str> {
    (0..3).find_map(|k| {
        let seed = (column * 7 + k * 131) as i32;
        let life = (t * (0.45 + 0.2 * hash(seed, 3)) + hash(seed, 9)).fract();
        let y = 0.35 + life * 0.75;
        let row = (y * rows as f32) as usize;
        (hash(seed, 1) > 0.82 && row == row_from_bottom && life < 0.85).then_some(if k % 2 == 0 {
            "·"
        } else {
            "'"
        })
    })
}

pub(super) fn selected_bar(effort: Effort, width: usize, rows: u16, t: f32) -> Vec<Line<'static>> {
    let rows = rows as usize;
    let mut lines = vec![Vec::with_capacity(width); rows];
    for column in 0..width {
        let x = column as f32 / (width.saturating_sub(1).max(1)) as f32;
        let level = effort_level(effort, x, t);
        let peak_eighths = (max_peak(x, t) * (rows * 8) as f32).round() as usize;
        let level_eighths = (level * (rows * 8) as f32).round() as usize;
        for (row, line) in lines.iter_mut().enumerate() {
            let from_bottom = rows - 1 - row;
            let cell_bottom = from_bottom as f32 / rows as f32;
            let cell_mid = (from_bottom as f32 + 0.5) / rows as f32;
            let fill = if effort == Effort::Extreme {
                let surface = flame_surface(x, cell_mid, t);
                (((surface - cell_bottom) * rows as f32 * 8.0)
                    .round()
                    .clamp(0.0, 8.0)) as usize
            } else {
                level_eighths.saturating_sub(from_bottom * 8).min(8)
            };
            let edge = if column == 0 || column + 1 == width {
                0.9
            } else {
                1.0
            };
            let span = if fill > 0 {
                let height = (from_bottom as f32 + fill as f32 / 8.0) / rows as f32;
                let color = if effort == Effort::Extreme {
                    let surface = flame_surface(x, cell_mid, t).max(0.05);
                    let heat = (1.0 - cell_bottom / surface) * surface.min(1.0) * 1.15;
                    fire_color(heat)
                } else {
                    lit_color(effort, x, height, t)
                };
                Span::styled(GLYPHS[fill], Style::default().fg(scale_color(color, edge)))
            } else if effort == Effort::Max
                && peak_eighths > level_eighths
                && (peak_eighths - 1) / 8 == from_bottom
            {
                Span::styled(
                    "▔",
                    Style::default().fg(hsv(x * 0.85 + t * 0.12, 0.25, 1.0)),
                )
            } else if effort == Effort::Extreme
                && let Some(spark) = ember_at(column, from_bottom, rows, t)
            {
                Span::styled(spark, Style::default().fg(fire_color(0.7)))
            } else {
                Span::raw(" ")
            };
            line.push(span);
        }
    }
    lines.into_iter().map(Line::from).collect()
}

fn idle_bar(
    index: usize,
    selected_index: usize,
    width: usize,
    rows: u16,
    t: f32,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(""); rows as usize - 1];
    let base = effort_rgb(index, 0, 1.0);
    let star = ((t * 0.8 + index as f32 * 3.0) as usize) % width.max(1);
    let twinkle = 0.18 + 0.2 * wave(t * 2.2 + index as f32 * 1.7);
    let glow = lit_color(LEVELS[selected_index], 0.5, 0.6, t);
    let spill_at = |column: usize| -> f32 {
        let distance = if index + 1 == selected_index {
            width - 1 - column
        } else if index == selected_index + 1 {
            column
        } else {
            return 0.0;
        };
        [0.42, 0.22].get(distance).copied().unwrap_or(0.0)
    };
    let spans = (0..width)
        .map(|column| {
            let (glyph, brightness) = if column == star {
                ("✦", twinkle)
            } else if column % 3 == 0 {
                ("·", 0.2)
            } else {
                (" ", 0.0)
            };
            let mut color = scale_color(base, brightness);
            let spill = spill_at(column);
            if spill > 0.0 {
                color = blend_color(color, glow, spill);
            }
            Span::styled(glyph, Style::default().fg(color))
        })
        .collect::<Vec<_>>();
    lines.push(Line::from(spans));
    lines
}

pub(super) fn draw_effort_picker(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    animation_tick: usize,
) {
    let t = app.launched_at.elapsed().as_secs_f32();
    let rows = bar_rows(area.height.saturating_sub(2));
    let height = (rows + CHROME_ROWS + 2).min(area.height);
    let width = (area.width as u32 * 96 / 100) as u16;
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Select effort ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(98, 213, 244)))
        .style(Style::default().bg(Color::Rgb(29, 30, 32)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width < LEVELS.len() as u16 || inner.height == 0 {
        return;
    }
    let row = |offset: u16, rows: u16| -> Option<Rect> {
        let y = inner.y + offset;
        (y + rows <= inner.bottom()).then(|| Rect::new(inner.x, y, inner.width, rows))
    };

    let column = |index: usize, area: Rect| {
        let left = area.x + area.width * index as u16 / LEVELS.len() as u16;
        let right = area.x + area.width * (index as u16 + 1) / LEVELS.len() as u16;
        Rect::new(left, area.y, right - left, area.height)
    };
    for index in 0..LEVELS.len() {
        let selected = index == app.picker_index;
        if let Some(line_area) = row(1, 1) {
            let label = if selected {
                let mut spans = vec![Span::styled("›", Style::default().fg(Color::White))];
                spans.extend(gradient_name(LEVELS[index], true, animation_tick));
                Line::from(spans)
            } else {
                Line::from(Span::styled(NAMES[index], Style::default().fg(Color::Gray)))
            };
            frame.render_widget(
                Paragraph::new(label).alignment(Alignment::Center),
                column(index, line_area),
            );
        }
        if let Some(line_area) = row(2, 1) {
            let color = if selected {
                lit_color(LEVELS[index], 0.5, 0.8, t)
            } else {
                Color::DarkGray
            };
            frame.render_widget(
                Paragraph::new(Span::styled(MAPPINGS[index], Style::default().fg(color)))
                    .alignment(Alignment::Center),
                column(index, line_area),
            );
        }
        if let Some(bar_area) = row(4, rows) {
            let cell = column(index, bar_area);
            let inner_width = cell.width.saturating_sub(2).max(1) as usize;
            let lines = if selected {
                selected_bar(LEVELS[index], inner_width, rows, t)
            } else {
                idle_bar(index, app.picker_index, inner_width, rows, t)
            };
            frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), cell);
        }
    }

    if let Some(description) = row(5 + rows, 2) {
        frame.render_widget(
            Paragraph::new(LEVELS[app.picker_index].description())
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            description,
        );
    }
    if let Some(footer) = row(7 + rows, 1) {
        frame.render_widget(
            Paragraph::new("←/→ move   Enter select   Esc cancel")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            footer,
        );
    }
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

    name.chars()
        .enumerate()
        .map(|(index, character)| {
            Span::styled(
                character.to_string(),
                Style::default()
                    .fg(colors[(index + animation_tick) % colors.len()])
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>()
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
    use super::{WHITE_HOT, bar_rows, effort_level, effort_rgb, lit_color, max_peak, selected_bar};
    use crate::tui::render::draw;
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
        for effort in [Effort::Max, Effort::XHigh, Effort::Super] {
            assert_ne!(
                lit_color(effort, 0.5, 0.6, 0.0),
                lit_color(effort, 0.5, 0.6, 0.7),
                "{effort:?} highlight should animate"
            );
        }
    }

    fn brightness(color: ratatui::style::Color) -> f32 {
        match color {
            ratatui::style::Color::Rgb(r, g, b) => (r as f32 + g as f32 + b as f32) / 3.0,
            _ => panic!("expected rgb"),
        }
    }

    #[test]
    fn calm_tiers_animate_their_light_subtly() {
        for (index, effort) in [Effort::Low, Effort::Medium, Effort::High]
            .into_iter()
            .enumerate()
        {
            let base = brightness(effort_rgb(index, 0, 1.0));
            let samples = (0..80)
                .map(|step| brightness(lit_color(effort, 0.4, 0.5, step as f32 * 0.1)))
                .collect::<Vec<_>>();
            let (min, max) = samples
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            assert!(max - min > 3.0, "{effort:?} should change over time");
            assert!(
                min >= base * 0.75 && max <= base * 1.3 + 1.0,
                "{effort:?} too strong: {min}..{max} around {base}"
            );
        }
    }

    const ALL: [Effort; 7] = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::XHigh,
        Effort::Max,
        Effort::Super,
        Effort::Extreme,
    ];

    #[test]
    fn effort_levels_stay_within_the_bar() {
        for effort in ALL {
            for step in 0..200 {
                let t = step as f32 * 0.037;
                for x in [0.0, 0.13, 0.5, 0.77, 1.0] {
                    let level = effort_level(effort, x, t);
                    assert!((0.0..=1.0).contains(&level), "{effort:?} {x} {t}: {level}");
                }
            }
        }
    }

    #[test]
    fn animations_move_smoothly_between_frames() {
        for effort in ALL {
            for step in 0..200 {
                let t = step as f32 * 0.05;
                for x in [0.0, 0.31, 0.62, 0.93] {
                    let delta =
                        (effort_level(effort, x, t + 0.05) - effort_level(effort, x, t)).abs();
                    assert!(delta < 0.15, "{effort:?} jumps by {delta} at x={x} t={t}");
                }
            }
        }
    }

    #[test]
    fn extreme_flickers_while_low_tiers_hold_still() {
        let samples =
            |effort| (0..20).map(move |step| effort_level(effort, 0.4, step as f32 * 0.25));
        let extreme = samples(Effort::Extreme).collect::<Vec<_>>();
        assert!(
            extreme
                .windows(2)
                .any(|pair| (pair[0] - pair[1]).abs() > 0.01)
        );
        for effort in [Effort::Low, Effort::Medium, Effort::High] {
            let values = samples(effort).collect::<Vec<_>>();
            assert!(
                values.windows(2).all(|pair| pair[0] == pair[1]),
                "{effort:?}"
            );
        }
    }

    #[test]
    fn higher_tiers_fill_more_of_the_bar() {
        let low = effort_level(Effort::Low, 0.5, 0.0);
        let medium = effort_level(Effort::Medium, 0.5, 0.0);
        let high = effort_level(Effort::High, 0.5, 0.0);
        assert!(low < medium && medium < high);
        let mean = |effort| {
            let samples =
                (0..400).map(|i| effort_level(effort, (i % 20) as f32 / 19.0, i as f32 * 0.07));
            samples.sum::<f32>() / 400.0
        };
        assert!(mean(Effort::Max) > high, "max mean {}", mean(Effort::Max));
    }

    #[test]
    fn xhigh_aurora_ripples_over_time() {
        let at = |t| effort_level(Effort::XHigh, 0.3, t);
        assert!((at(0.0) - at(1.0)).abs() > 0.02);
    }

    #[test]
    fn max_peak_caps_sit_at_or_above_the_bar() {
        for step in 0..300 {
            let t = step as f32 * 0.043;
            for x in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert!(max_peak(x, t) >= effort_level(Effort::Max, x, t) - 1e-6);
                assert!(max_peak(x, t) <= 1.0);
            }
        }
    }

    fn extreme_frames() -> Vec<Vec<ratatui::text::Line<'static>>> {
        (0..60)
            .map(|step| selected_bar(Effort::Extreme, 30, 4, step as f32 * 0.11))
            .collect()
    }

    #[test]
    fn extreme_fire_throws_embers_above_the_flames() {
        let ember = extreme_frames()
            .iter()
            .flatten()
            .flat_map(|line| &line.spans)
            .any(|span| span.content == "·" || span.content == "'");
        assert!(ember);
    }

    #[test]
    fn extreme_fire_has_a_white_hot_core() {
        let hot = extreme_frames()
            .iter()
            .flatten()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.fg == Some(WHITE_HOT));
        assert!(hot);
    }

    #[test]
    fn bar_uses_more_rows_when_space_allows() {
        assert_eq!(bar_rows(30), 4);
        assert_eq!(bar_rows(11), 2);
        assert_eq!(bar_rows(4), 1);
        assert_eq!(bar_rows(0), 1);
    }

    #[test]
    fn effort_picker_draws_on_tiny_terminals() {
        for (width, height) in [(20, 5), (30, 8), (60, 12), (120, 40)] {
            let mut app = App::new(Settings::default());
            app.trust_prompt = false;
            app.picker = true;
            app.picker_index = 6;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            terminal
                .draw(|frame| draw(frame, &app, 0))
                .unwrap_or_else(|_| panic!("{width}x{height}"));
        }
    }
}
