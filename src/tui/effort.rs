use crate::Effort;
use crate::tui::state::App;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

const GLYPHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
// Rows used by everything except the bar: padding, labels, mappings, gaps, the workflows
// checkbox, description, footer.
const CHROME_ROWS: u16 = 10;

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
        // The void's edge breathes slowly; the noise keeps neighbouring columns from moving in step.
        Effort::Ultimate => 0.74 + 0.4 * (smooth_noise(x * 4.0, t * 0.5) - 0.5),
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
        Effort::Ultimate => blend_color(
            Color::Rgb(150, 110, 235),
            Color::Rgb(250, 248, 255),
            wave(x * 6.0 - t * 1.4) * (0.4 + 0.6 * height),
        ),
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

// The deepest, darkest violet of the void.
const VOID_BASE: (u8, u8, u8) = (8, 6, 18);
// Where the dark-matter clouds are thickest.
const VOID_CLOUD: (u8, u8, u8) = (56, 26, 98);

/// Background of a void cell: near black with slow clouds of dark matter drifting through it.
fn void_color(column: usize, from_bottom: usize, t: f32) -> Color {
    let cloud = fbm(
        column as f32 * 0.22 + t * 0.12,
        from_bottom as f32 * 0.8 - t * 0.07,
    );
    blend_color(
        Color::Rgb(VOID_BASE.0, VOID_BASE.1, VOID_BASE.2),
        Color::Rgb(VOID_CLOUD.0, VOID_CLOUD.1, VOID_CLOUD.2),
        (cloud * cloud * 1.6).min(1.0) * 0.85,
    )
}

/// A star inside the void: `None` while it is dark, otherwise its glyph and color.
fn void_star(column: usize, from_bottom: usize, t: f32) -> Option<(&'static str, Color)> {
    let (x, y) = (column as i32, from_bottom as i32);
    if hash(x * 3 + 1, y * 5 + 2) < 0.86 {
        return None;
    }
    let pace = 0.9 + 2.2 * hash(x, y + 40);
    let phase = hash(x + 90, y) * std::f32::consts::TAU;
    let brightness = wave(t * pace + phase);
    if brightness < 0.2 {
        return None;
    }
    let glyph = match brightness {
        b if b < 0.5 => "·",
        b if b < 0.85 => "✧",
        _ => "✦",
    };
    let color = blend_color(
        Color::Rgb(120, 112, 175),
        Color::Rgb(255, 255, 255),
        brightness,
    );
    Some((glyph, color))
}

/// Ultimate: a dark-matter void with white stars and a softly glowing edge.
fn void_bar(width: usize, rows: u16, t: f32) -> Vec<Line<'static>> {
    let rows = rows as usize;
    let mut lines = vec![Vec::with_capacity(width); rows];
    for column in 0..width {
        let x = column as f32 / (width.saturating_sub(1).max(1)) as f32;
        let level_eighths =
            (effort_level(Effort::Ultimate, x, t) * (rows * 8) as f32).round() as usize;
        let top_row = level_eighths.div_ceil(8).saturating_sub(1);
        for (row, line) in lines.iter_mut().enumerate() {
            let from_bottom = rows - 1 - row;
            let fill = level_eighths.saturating_sub(from_bottom * 8).min(8);
            let span = if fill == 0 {
                Span::raw(" ")
            } else if from_bottom == top_row {
                // The rim: light bending around the edge of the void.
                let glow = blend_color(
                    Color::Rgb(124, 88, 226),
                    Color::Rgb(255, 255, 255),
                    0.45 + 0.4 * wave(x * 9.0 - t * 1.6),
                );
                Span::styled(GLYPHS[fill], Style::default().fg(glow))
            } else {
                let background = void_color(column, from_bottom, t);
                match void_star(column, from_bottom, t) {
                    Some((glyph, color)) => {
                        Span::styled(glyph, Style::default().fg(color).bg(background))
                    }
                    None => Span::styled(" ", Style::default().bg(background)),
                }
            };
            line.push(span);
        }
    }
    lines.into_iter().map(Line::from).collect()
}

pub(super) fn selected_bar(effort: Effort, width: usize, rows: u16, t: f32) -> Vec<Line<'static>> {
    if effort == Effort::Ultimate {
        return void_bar(width, rows, t);
    }
    let rows = rows as usize;
    let mut lines = vec![Vec::with_capacity(width); rows];
    for column in 0..width {
        let x = column as f32 / (width.saturating_sub(1).max(1)) as f32;
        let level = effort_level(effort, x, t);
        let peak_eighths = (max_peak(x, t) * (rows * 8) as f32).round() as usize;
        let level_eighths = (level * (rows * 8) as f32).round() as usize;
        for (row, line) in lines.iter_mut().enumerate() {
            let from_bottom = rows - 1 - row;
            let fill = level_eighths.saturating_sub(from_bottom * 8).min(8);
            let edge = if column == 0 || column + 1 == width {
                0.9
            } else {
                1.0
            };
            let span = if fill > 0 {
                let height = (from_bottom as f32 + fill as f32 / 8.0) / rows as f32;
                let color = lit_color(effort, x, height, t);
                Span::styled(GLYPHS[fill], Style::default().fg(scale_color(color, edge)))
            } else if effort == Effort::Max
                && peak_eighths > level_eighths
                && (peak_eighths - 1) / 8 == from_bottom
            {
                Span::styled(
                    "▔",
                    Style::default().fg(hsv(x * 0.85 + t * 0.12, 0.25, 1.0)),
                )
            } else {
                Span::raw(" ")
            };
            line.push(span);
        }
    }
    lines.into_iter().map(Line::from).collect()
}

/// How far above the void a star can climb, in rows.
const STAR_CLIMB_ROWS: f32 = 16.0;
const RISING_STARS: u32 = 18;

/// One star rising out of the void: where it is now and how brightly it shines (0..=1).
struct RisingStar {
    x: u16,
    y: u16,
    brightness: f32,
}

/// Stars launched from the top of the void in `origin` that float upward, out of the popup, and
/// fade as they climb. `ceiling` is the highest row they may reach.
fn rising_stars(origin: Rect, ceiling: u16, t: f32) -> Vec<RisingStar> {
    if origin.width == 0 || origin.y <= ceiling {
        return Vec::new();
    }
    let climb = f32::from(origin.y - ceiling).min(STAR_CLIMB_ROWS);
    (0..RISING_STARS as i32)
        .filter_map(|index| {
            let speed = 0.18 + 0.3 * hash(index, 1);
            let life = (t * speed + hash(index, 2)).fract();
            let sway = (t * 0.8 + hash(index, 3) * 6.3).sin() * 1.2;
            let x = f32::from(origin.x) + hash(index, 4) * f32::from(origin.width) + sway;
            let y = f32::from(origin.y) + 0.5 - life * climb;
            let brightness = (1.0 - life).powf(0.9);
            (x >= 0.0 && y >= f32::from(ceiling) && brightness >= 0.1).then_some(RisingStar {
                x: x as u16,
                y: y as u16,
                brightness,
            })
        })
        .collect()
}

/// Paints the rising stars over empty cells and the popup's plain border, so they appear to
/// leave the box; text is never covered.
fn draw_rising_stars(frame: &mut ratatui::Frame<'_>, origin: Rect, t: f32) {
    let screen = frame.area();
    let buffer = frame.buffer_mut();
    for star in rising_stars(origin, screen.y, t) {
        if star.x >= screen.right() || star.y >= screen.bottom() {
            continue;
        }
        let cell = &mut buffer[(star.x, star.y)];
        if cell.symbol() != " " && cell.symbol() != "─" {
            continue;
        }
        let glyph = match star.brightness {
            b if b > 0.75 => "✦",
            b if b > 0.4 => "✧",
            _ => "·",
        };
        cell.set_symbol(glyph).set_fg(blend_color(
            Color::Rgb(70, 62, 120),
            Color::Rgb(255, 255, 255),
            star.brightness,
        ));
    }
}

fn idle_bar(
    index: usize,
    selected_index: usize,
    effort: Effort,
    selected_effort: Effort,
    width: usize,
    rows: u16,
    t: f32,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(""); rows as usize - 1];
    let base = effort_color(effort, 1.0);
    let star = ((t * 0.8 + index as f32 * 3.0) as usize) % width.max(1);
    let twinkle = 0.18 + 0.2 * wave(t * 2.2 + index as f32 * 1.7);
    let glow = lit_color(selected_effort, 0.5, 0.6, t);
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

/// The label under a column: what that choice actually does for the active model.
fn mapping_label(
    effort: Effort,
    model_levels: &[Effort],
    unlocked: bool,
    workflows_ticked: bool,
) -> String {
    let base = match crate::effort_support::nearest_level(effort.model_level(), model_levels) {
        Some(level) => effort_name(level),
        None => "default".to_owned(),
    };
    if effort.is_workflow_tier() {
        if unlocked {
            format!("{base}+wf")
        } else {
            "locked".to_owned()
        }
    } else if workflows_ticked && matches!(effort, Effort::Low | Effort::Medium | Effort::High) {
        format!("{base}+wf")
    } else {
        base
    }
}

const WORKFLOWS_NOTE: &str = "Workflows let the model split work across subagents and have the result reviewed. They can use many times more tokens.";

impl App {
    /// The effort levels of the active model, lowest first; every level when no model is chosen
    /// yet (nothing to hide), none when the model has no adjustable effort.
    pub(super) fn active_model_levels(&self) -> Vec<Effort> {
        match self.settings.model.as_deref() {
            Some(model) => crate::effort_support::supported_levels(model, &[]),
            None => crate::effort_support::MODEL_LEVELS.to_vec(),
        }
    }

    /// The columns of the effort picker: the model's own levels, then the workflow tiers it can
    /// carry (shown even while locked, so they can be discovered). Super is XHigh with workflows
    /// and Ultimate is Max with workflows, so a model without XHigh or Max has neither.
    pub(super) fn picker_levels(&self) -> Vec<Effort> {
        let mut levels = self.active_model_levels();
        if levels.contains(&Effort::XHigh) {
            levels.push(Effort::Super);
        }
        if levels.contains(&Effort::Max) {
            levels.push(Effort::Ultimate);
        }
        levels
    }

    /// Opens the picker on the current effort (or the nearest column the model has).
    pub(super) fn open_effort_picker(&mut self) {
        let levels = self.picker_levels();
        if levels.is_empty() {
            self.notice = "This model has no adjustable effort.".to_owned();
            return;
        }
        let wanted_rank = rank_of(self.settings.effort);
        self.picker_index = levels
            .iter()
            .position(|level| *level == self.settings.effort)
            .or_else(|| {
                levels
                    .iter()
                    .rposition(|level| !level.is_workflow_tier() && rank_of(*level) <= wanted_rank)
            })
            .unwrap_or(0);
        self.picker_workflows = self.settings.workflows;
        self.picker_focus_workflows = false;
        self.picker = true;
    }

    pub(super) fn handle_effort_picker_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> anyhow::Result<()> {
        use crossterm::event::KeyCode;
        let levels = self.picker_levels();
        let last = levels.len() - 1;
        self.picker_index = self.picker_index.min(last);
        let current = levels[self.picker_index];
        let plain = matches!(current, Effort::Low | Effort::Medium | Effort::High);
        match key.code {
            KeyCode::Left => {
                self.picker_index = self.picker_index.saturating_sub(1);
                self.picker_focus_workflows = false;
            }
            KeyCode::Right => {
                self.picker_index = (self.picker_index + 1).min(last);
                self.picker_focus_workflows = false;
            }
            KeyCode::Down if plain && !self.picker_focus_workflows => {
                if self.settings.workflows_unlocked() {
                    self.picker_focus_workflows = true;
                } else {
                    self.notice =
                        "Workflows are locked. Turn on Dynamic workflows in Settings → General."
                            .to_owned();
                }
            }
            KeyCode::Down if matches!(current, Effort::XHigh | Effort::Max) => {
                let tier = if current == Effort::XHigh {
                    Effort::Super
                } else {
                    Effort::Ultimate
                };
                if let Some(index) = levels.iter().position(|level| *level == tier) {
                    if self.settings.workflows_unlocked() {
                        self.picker_index = index;
                    } else {
                        self.notice = format!(
                            "{} is locked. Turn on Dynamic workflows in Settings → General.",
                            effort_name(tier)
                        );
                    }
                }
            }
            KeyCode::Up if self.picker_focus_workflows => self.picker_focus_workflows = false,
            KeyCode::Up if current.is_workflow_tier() => {
                let base = current.model_level();
                if let Some(index) = levels.iter().position(|level| *level == base) {
                    self.picker_index = index;
                }
            }
            KeyCode::Char(' ') if self.picker_focus_workflows => {
                self.picker_workflows = !self.picker_workflows;
            }
            KeyCode::Enter if self.picker_focus_workflows => {
                self.picker_workflows = !self.picker_workflows;
            }
            KeyCode::Enter => self.choose_effort()?,
            KeyCode::Esc => {
                self.picker = false;
                self.picker_focus_workflows = false;
                self.notice = "Effort unchanged.".to_owned();
            }
            _ => {}
        }
        Ok(())
    }
}

/// Where a level sits among the five model levels (workflow tiers count as their base).
fn rank_of(effort: Effort) -> usize {
    crate::effort_support::MODEL_LEVELS
        .iter()
        .position(|level| *level == effort.model_level())
        .unwrap_or(0)
}

pub(super) fn draw_effort_picker(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    animation_tick: usize,
) {
    let levels = app.picker_levels();
    let selected_index = app.picker_index.min(levels.len() - 1);
    let selected_effort = levels[selected_index];
    let model_levels = app.active_model_levels();
    let unlocked = app.settings.workflows_unlocked();
    let on_plain_level = matches!(selected_effort, Effort::Low | Effort::Medium | Effort::High);
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
        .border_style(Style::default().fg(crate::tui::theme::accent()))
        .style(Style::default().bg(crate::tui::theme::panel()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width < levels.len() as u16 || inner.height == 0 {
        return;
    }
    let row = |offset: u16, rows: u16| -> Option<Rect> {
        let y = inner.y + offset;
        (y + rows <= inner.bottom()).then(|| Rect::new(inner.x, y, inner.width, rows))
    };
    let count = levels.len() as u16;
    let column = |index: usize, area: Rect| {
        let left = area.x + area.width * index as u16 / count;
        let right = area.x + area.width * (index as u16 + 1) / count;
        Rect::new(left, area.y, right - left, area.height)
    };
    for (index, effort) in levels.iter().copied().enumerate() {
        let selected = index == selected_index;
        let locked = effort.is_workflow_tier() && !unlocked;
        let ticked = selected && app.picker_workflows && on_plain_level;
        if let Some(line_area) = row(1, 1) {
            let label = if selected && !locked {
                let mut spans = vec![Span::styled("›", Style::default().fg(Color::White))];
                spans.extend(gradient_name(effort, true, animation_tick));
                Line::from(spans)
            } else if selected {
                Line::from(vec![
                    Span::styled("›", Style::default().fg(Color::White)),
                    Span::styled(effort_label(effort), Style::default().fg(Color::Gray)),
                ])
            } else {
                Line::from(Span::styled(
                    effort_label(effort),
                    Style::default().fg(if locked { Color::DarkGray } else { Color::Gray }),
                ))
            };
            frame.render_widget(
                Paragraph::new(label).alignment(Alignment::Center),
                column(index, line_area),
            );
        }
        if let Some(line_area) = row(2, 1) {
            let color = if ticked {
                Color::Rgb(255, 197, 92)
            } else if selected && !locked {
                lit_color(effort, 0.5, 0.8, t)
            } else {
                Color::DarkGray
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    mapping_label(effort, &model_levels, unlocked, ticked),
                    Style::default().fg(color),
                ))
                .alignment(Alignment::Center),
                column(index, line_area),
            );
        }
        if let Some(bar_area) = row(4, rows) {
            let cell = column(index, bar_area);
            let inner_width = cell.width.saturating_sub(2).max(1) as usize;
            let lines = if selected {
                selected_bar(effort, inner_width, rows, t)
            } else {
                idle_bar(
                    index,
                    selected_index,
                    effort,
                    selected_effort,
                    inner_width,
                    rows,
                    t,
                )
            };
            frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), cell);
        }
    }

    // Ultimate is the one tier that escapes its box: stars lift off the void and leave the popup.
    if selected_effort == Effort::Ultimate
        && unlocked
        && let Some(bar_area) = row(4, rows)
    {
        draw_rising_stars(frame, column(selected_index, bar_area), t);
    }

    // The workflows checkbox sits under the Low, Medium and High columns.
    if on_plain_level && let Some(line_area) = row(5 + rows, 1) {
        let focused = app.picker_focus_workflows;
        let (mark, color) = if !unlocked {
            ("[ ] Workflows (locked)", Color::DarkGray)
        } else if app.picker_workflows {
            ("[x] Workflows ON: more tokens", Color::Rgb(255, 197, 92))
        } else {
            ("[ ] Workflows", Color::Gray)
        };
        let style = if focused {
            Style::default()
                .fg(color)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(color)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {mark} "), style)).alignment(Alignment::Center),
            line_area,
        );
    }

    if let Some(description) = row(6 + rows, 2) {
        let mut text = if app.picker_focus_workflows {
            WORKFLOWS_NOTE.to_owned()
        } else if selected_effort.is_workflow_tier() && !unlocked {
            "Locked: turn on Dynamic workflows in Settings → General to use it.".to_owned()
        } else {
            selected_effort.description().to_owned()
        };
        if selected_effort.is_workflow_tier() && unlocked {
            let budget = crate::workflow::Budget::new(
                selected_effort,
                app.settings.workflow_limit(),
                app.settings.subagents_at_once(),
            );
            text.push_str(&format!(
                ". Up to {} subagents this turn ({} at once).",
                budget.total_runs, budget.at_once
            ));
        }
        if model_levels.is_empty() {
            text.push_str(" This model has no adjustable effort levels.");
        }
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            description,
        );
    }
    if let Some(footer) = row(8 + rows, 1) {
        let hint = if app.picker_focus_workflows {
            "Space tick   ↑ back   Enter select   Esc cancel"
        } else if on_plain_level {
            "←/→ move   ↓ workflows   Enter select   Esc cancel"
        } else if matches!(selected_effort, Effort::XHigh | Effort::Max) {
            "←/→ move   ↓ workflow tier   Enter select   Esc cancel"
        } else {
            "←/→ move   Enter select   Esc cancel"
        };
        frame.render_widget(
            Paragraph::new(hint)
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
        Effort::Ultimate => Color::LightMagenta,
        _ => Color::White,
    };
    let style = Style::default().fg(color);
    if selected {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

pub(super) fn effort_color(effort: Effort, brightness: f32) -> Color {
    let color = match effort {
        Effort::Low => (137, 148, 164),
        Effort::Medium => (94, 148, 235),
        Effort::High => (65, 197, 214),
        Effort::Max => (190, 105, 210),
        Effort::XHigh => (155, 125, 240),
        Effort::Super => (241, 184, 63),
        Effort::Ultimate => (170, 140, 255),
    };
    scale_rgb(color, brightness)
}

/// The color of the effort at `index` in the full seven-level order.
pub(super) fn effort_rgb(index: usize, _animation_tick: usize, brightness: f32) -> Color {
    effort_color(crate::tui::state::LEVELS[index], brightness)
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

/// The colors an advanced tier's name cycles through.
fn gradient_colors(effort: Effort) -> &'static [Color] {
    match effort {
        Effort::Max => &[
            Color::Rgb(255, 85, 85),
            Color::Rgb(255, 128, 0),
            Color::Rgb(255, 235, 90),
            Color::Rgb(90, 220, 110),
            Color::Rgb(80, 220, 230),
            Color::Rgb(90, 130, 255),
            Color::Rgb(225, 100, 225),
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
            Color::Rgb(255, 235, 90),
            Color::Rgb(255, 190, 0),
            Color::Rgb(230, 145, 0),
            Color::Rgb(255, 225, 100),
        ],
        Effort::Ultimate => &[
            Color::Rgb(255, 255, 255),
            Color::Rgb(205, 190, 255),
            Color::Rgb(150, 120, 235),
            Color::Rgb(98, 70, 185),
            Color::Rgb(150, 120, 235),
            Color::Rgb(205, 190, 255),
        ],
        _ => &[],
    }
}

pub(super) fn gradient_name(
    effort: Effort,
    selected: bool,
    animation_tick: usize,
) -> Vec<Span<'static>> {
    let colors = gradient_colors(effort);
    if !selected || colors.is_empty() {
        return vec![Span::styled(
            effort_label(effort).to_owned(),
            effort_style(effort, selected),
        )];
    }
    effort_label(effort)
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
        .collect()
}

/// How long the effort name in the status line shines after a change.
pub(super) const FLASH_SECONDS: f32 = 2.0;
/// The last part of that time is a gradual fade back to the plain color.
const FADE_SECONDS: f32 = 1.2;

/// How strongly the gradient still shows with `remaining` of the flash left: full until the
/// fade begins, then easing smoothly down to nothing.
pub(super) fn flash_strength(remaining: std::time::Duration) -> f32 {
    let x = (remaining.as_secs_f32() / FADE_SECONDS).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The tier's name with its gradient blended toward the plain color by `1 - strength`.
pub(super) fn faded_gradient_name(
    effort: Effort,
    animation_tick: usize,
    strength: f32,
) -> Vec<Span<'static>> {
    let colors = gradient_colors(effort);
    let plain = effort_color(effort, 1.0);
    // Lower-case, like the settled name, so only the colors change as it fades.
    effort_name(effort)
        .chars()
        .enumerate()
        .map(|(index, character)| {
            let color = match colors.get((index + animation_tick) % colors.len().max(1)) {
                Some(shining) => blend_color(plain, *shining, strength),
                None => plain,
            };
            Span::styled(
                character.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// The effort as the status line shows it: the name (shining, fading or always animated), a
/// `+wf` tag when workflows are on, or a note when the model has no effort control.
pub(super) fn status_effort_spans(
    app: &App,
    animation_tick: usize,
    now: std::time::Instant,
) -> Vec<Span<'static>> {
    let effort = app.settings.effort;
    let strength = if app.settings.effort_always_animated {
        1.0
    } else {
        app.effort_flash_until.map_or(0.0, |until| {
            flash_strength(until.saturating_duration_since(now))
        })
    };
    let mut spans = if app.active_model_levels().is_empty() && !effort.is_workflow_tier() {
        vec![Span::styled(
            "no effort",
            Style::default().fg(Color::DarkGray),
        )]
    } else if strength > 0.0 && !gradient_colors(effort).is_empty() {
        faded_gradient_name(effort, animation_tick, strength)
    } else {
        let mut style = Style::default().fg(effort_color(effort, 1.0));
        if strength > 0.0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        vec![Span::styled(effort_name(effort), style)]
    };
    if app.settings.workflows_active() && !effort.is_workflow_tier() {
        spans.push(Span::styled(
            " +wf",
            Style::default()
                .fg(Color::Rgb(255, 197, 92))
                .add_modifier(Modifier::BOLD),
        ));
    }
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
        Effort::Ultimate => "Ultimate",
    }
}

#[cfg(test)]
mod tests {
    use super::{bar_rows, effort_level, effort_rgb, lit_color, max_peak, selected_bar};
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
        for expected in ["Low", "Medium", "High", "XHigh", "Max", "Super", "Ultimate"] {
            assert!(
                rendered.contains(expected),
                "missing {expected} in picker:\n{rendered}"
            );
        }
        assert!(
            rendered.contains("locked") && !rendered.contains("xhigh+wf"),
            "the workflow tiers say they are locked by default:\n{rendered}"
        );
        app.settings.workflow_size = crate::workflow::WorkflowSize::Medium;
        terminal
            .draw(|frame| draw(frame, &app, 0))
            .expect("draw unlocked");
        let unlocked = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            unlocked.contains("xhigh+wf") && unlocked.contains("max+wf"),
            "{unlocked}"
        );
        let ordered = ["Low", "Medium", "High", "XHigh", "Max", "Super", "Ultimate"]
            .map(|label| rendered.find(label).expect("effort label"));
        assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn the_picker_says_how_many_subagents_a_workflow_tier_may_start() {
        let mut terminal = Terminal::new(TestBackend::new(110, 32)).expect("test terminal");
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.settings.workflow_size = crate::workflow::WorkflowSize::Large;
        app.picker = true;
        let shown = |app: &App, terminal: &mut Terminal<TestBackend>| {
            terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        app.picker_index = app.picker_levels().len() - 1;
        let ultimate = shown(&app, &mut terminal);
        assert!(ultimate.contains("Up to 50 subagents"), "{ultimate}");
        app.picker_index = app.picker_levels().len() - 2;
        let super_tier = shown(&app, &mut terminal);
        assert!(super_tier.contains("Up to 25 subagents"), "{super_tier}");
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
        Effort::Ultimate,
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
        let extreme = samples(Effort::Ultimate).collect::<Vec<_>>();
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

    fn void_frames() -> Vec<Vec<ratatui::text::Line<'static>>> {
        (0..80)
            .map(|step| selected_bar(Effort::Ultimate, 30, 4, step as f32 * 0.13))
            .collect()
    }

    fn spans<'a>(
        frames: &'a [Vec<ratatui::text::Line<'static>>],
    ) -> impl Iterator<Item = &'a ratatui::text::Span<'static>> {
        frames.iter().flatten().flat_map(|line| &line.spans)
    }

    fn rgb(color: Option<ratatui::style::Color>) -> (u8, u8, u8) {
        match color {
            Some(ratatui::style::Color::Rgb(r, g, b)) => (r, g, b),
            other => panic!("expected an RGB color, got {other:?}"),
        }
    }

    const STARS: [&str; 3] = ["·", "✧", "✦"];

    #[test]
    fn the_void_is_scattered_with_white_stars() {
        let frames = void_frames();
        let bright = spans(&frames)
            .filter(|span| STARS.contains(&span.content.as_ref()))
            .filter(|span| {
                let (r, g, b) = rgb(span.style.fg);
                r > 215 && g > 215 && b > 225
            })
            .count();
        assert!(bright > 20, "{bright} white stars in 80 frames");
    }

    #[test]
    fn the_stars_twinkle_instead_of_holding_still() {
        let frames = void_frames();
        let at = |frame: usize| {
            frames[frame]
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let distinct = (0..frames.len())
            .map(at)
            .collect::<std::collections::HashSet<_>>();
        assert!(distinct.len() > 40, "{} distinct frames", distinct.len());
        let shades = spans(&frames)
            .filter(|span| STARS.contains(&span.content.as_ref()))
            .map(|span| rgb(span.style.fg))
            .collect::<std::collections::HashSet<_>>();
        assert!(shades.len() > 8, "{} star shades", shades.len());
    }

    #[test]
    fn the_void_itself_is_dark_and_has_no_fire() {
        let frames = void_frames();
        for span in spans(&frames) {
            let Some(background) = span.style.bg else {
                continue;
            };
            let (r, g, b) = rgb(Some(background));
            assert!(r < 80 && g < 50 && b < 130, "void cell {r},{g},{b}");
            assert!(b >= r, "the void leans violet, not red: {r},{g},{b}");
        }
        assert!(
            !spans(&frames).any(|span| {
                span.style.fg.is_some_and(|color| {
                    let (r, g, b) = rgb(Some(color));
                    r > 200 && g < 140 && b < 90
                })
            }),
            "nothing orange or red remains"
        );
    }

    #[test]
    fn the_edge_of_the_void_glows_brighter_than_its_depths() {
        let frames = void_frames();
        let brightness = |color: Option<ratatui::style::Color>| {
            let (r, g, b) = rgb(color);
            r as u32 + g as u32 + b as u32
        };
        // The topmost lit cell of a column is its rim: a bright, bg-less glyph.
        let rims = spans(&frames)
            .filter(|span| span.style.bg.is_none() && span.content.as_ref() != " ")
            .filter(|span| !STARS.contains(&span.content.as_ref()))
            .map(|span| brightness(span.style.fg))
            .collect::<Vec<_>>();
        assert!(!rims.is_empty());
        let mean = rims.iter().sum::<u32>() as f32 / rims.len() as f32;
        assert!(mean > 330.0, "rim brightness {mean}");
    }

    #[test]
    fn the_void_bar_has_the_requested_shape() {
        for (width, rows) in [(1, 1), (3, 1), (30, 4), (60, 2)] {
            let lines = selected_bar(Effort::Ultimate, width, rows, 1.0);
            assert_eq!(lines.len(), rows as usize);
            assert!(lines.iter().all(|line| line.spans.len() == width));
        }
    }

    #[test]
    fn stars_rise_out_of_the_void_and_fade_with_height() {
        let origin = ratatui::layout::Rect::new(10, 30, 12, 4);
        let mut near = Vec::new();
        let mut far = Vec::new();
        let mut highest = 30;
        for step in 0..400 {
            for star in super::rising_stars(origin, 2, step as f32 * 0.1) {
                assert!(star.y >= 2 && star.y <= 30 + 1, "row {}", star.y);
                assert!(
                    star.x + 2 >= origin.x && star.x <= origin.right() + 2,
                    "column {}",
                    star.x
                );
                assert!((0.0..=1.0).contains(&star.brightness));
                highest = highest.min(star.y);
                match 30 - i32::from(star.y) {
                    ..=3 => near.push(star.brightness),
                    9.. => far.push(star.brightness),
                    _ => {}
                }
            }
        }
        assert!(highest <= 30 - 12, "stars only reached row {highest}");
        assert!(!near.is_empty() && !far.is_empty());
        let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len() as f32;
        assert!(
            mean(&far) < mean(&near) * 0.6,
            "{} vs {}",
            mean(&far),
            mean(&near)
        );
    }

    #[test]
    fn stars_never_climb_above_the_ceiling_or_off_a_tiny_screen() {
        for step in 0..100 {
            for star in
                super::rising_stars(ratatui::layout::Rect::new(0, 3, 5, 1), 2, step as f32 * 0.3)
            {
                assert!(star.y >= 2);
            }
        }
        assert!(
            super::rising_stars(ratatui::layout::Rect::new(0, 0, 5, 1), 0, 1.0)
                .iter()
                .all(|s| s.y == 0)
        );
        assert!(super::rising_stars(ratatui::layout::Rect::new(0, 5, 0, 0), 0, 1.0).is_empty());
    }

    fn star_cells_in_view(effort_index: usize) -> usize {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.settings.workflow_size = crate::workflow::WorkflowSize::Medium;
        app.settings.background_animation = false;
        app.transcript.push(crate::tui::state::TranscriptEntry {
            kind: crate::tui::state::TranscriptKind::Assistant,
            text: "hello".to_owned(),
        });
        app.picker = true;
        app.picker_index = effort_index;
        let mut terminal = Terminal::new(TestBackend::new(120, 44)).expect("terminal");
        let mut best = 0;
        for step in 0..30 {
            app.launched_at =
                std::time::Instant::now() - std::time::Duration::from_millis(step * 250);
            terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
            let count = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .enumerate()
                // Only the rows above the popup, where nothing else draws stars.
                .filter(|(index, _)| *index < 120 * 8)
                .filter(|(_, cell)| ["✦", "✧", "·"].contains(&cell.symbol()))
                .count();
            best = best.max(count);
        }
        best
    }

    #[test]
    fn only_ultimate_throws_stars_out_of_the_box() {
        let ultimate = star_cells_in_view(6);
        let high = star_cells_in_view(2);
        assert!(ultimate >= high + 3, "ultimate {ultimate}, high {high}");
    }

    fn picker_app(model: Option<&str>, dynamic: bool, effort: Effort) -> App {
        let mut settings = Settings::default();
        settings.model = model.map(str::to_owned);
        settings.workflow_size = if dynamic {
            crate::workflow::WorkflowSize::Medium
        } else {
            crate::workflow::WorkflowSize::Off
        };
        settings.effort = effort;
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.open_effort_picker();
        app
    }

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn current(app: &App) -> Effort {
        app.picker_levels()[app.picker_index]
    }

    fn screen_of(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 34)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn a_model_only_offers_the_levels_it_has() {
        let deepseek = picker_app(Some("deepseek-chat"), false, Effort::High);
        assert!(
            deepseek.picker_levels().is_empty(),
            "no effort, so no tiers either"
        );
        assert!(!deepseek.picker, "there is nothing to pick");
        assert_eq!(deepseek.notice, "This model has no adjustable effort.");
        let gemini = picker_app(Some("gemini-3-pro"), false, Effort::High);
        assert_eq!(
            gemini.picker_levels(),
            [Effort::Low, Effort::High],
            "no XHigh means no Super, no Max means no Ultimate"
        );
        let gpt_5_5 = picker_app(Some("gpt-5.5"), false, Effort::High);
        assert_eq!(
            gpt_5_5.picker_levels(),
            [
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::XHigh,
                Effort::Super
            ],
            "XHigh without Max gives Super but not Ultimate"
        );
        let gpt_6 = picker_app(Some("gpt-6-astra"), false, Effort::High);
        assert_eq!(gpt_6.picker_levels().len(), 7, "GPT-6 has Max");
        let claude = picker_app(Some("claude-opus-5-5"), false, Effort::Max);
        assert_eq!(claude.picker_levels().len(), 7);
        let nothing_chosen = picker_app(None, false, Effort::High);
        assert_eq!(
            nothing_chosen.picker_levels().len(),
            7,
            "nothing to hide yet"
        );
    }

    #[test]
    fn the_picker_opens_on_the_current_effort_or_the_nearest_column() {
        assert_eq!(
            current(&picker_app(Some("claude-opus-5-5"), false, Effort::XHigh)),
            Effort::XHigh
        );
        // Max is not available on this model, so the picker opens on its best level.
        let capped = picker_app(Some("gpt-5.5"), false, Effort::Max);
        assert_eq!(current(&capped), Effort::XHigh);
        let low_only = picker_app(Some("gemini-3-pro"), false, Effort::Medium);
        assert_eq!(
            current(&low_only),
            Effort::Low,
            "rounds down to a level that exists"
        );
    }

    #[test]
    fn down_on_a_plain_level_reaches_the_workflows_checkbox_and_space_ticks_it() {
        use crossterm::event::KeyCode;
        let mut app = picker_app(Some("claude-opus-5-5"), true, Effort::High);
        assert!(!app.picker_focus_workflows);
        app.handle_effort_picker_key(key(KeyCode::Down)).unwrap();
        assert!(app.picker_focus_workflows, "the checkbox has the cursor");
        assert!(screen_of(&app).contains("[ ] Workflows"));
        app.handle_effort_picker_key(key(KeyCode::Char(' ')))
            .unwrap();
        assert!(app.picker_workflows);
        let shown = screen_of(&app);
        assert!(
            shown.contains("Workflows ON") && shown.contains("high+wf"),
            "{shown}"
        );
        app.handle_effort_picker_key(key(KeyCode::Up)).unwrap();
        assert!(!app.picker_focus_workflows);
        assert!(app.picker_workflows, "the tick is kept");
        app.handle_effort_picker_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(app.settings.effort, Effort::High);
        assert!(app.settings.workflows, "High now runs with workflows");
        assert!(app.settings.workflows_active());
    }

    #[test]
    fn moving_off_the_checkbox_with_arrows_and_confirming_applies_the_tick() {
        use crossterm::event::KeyCode;
        let mut app = picker_app(Some("claude-opus-5-5"), true, Effort::Medium);
        app.handle_effort_picker_key(key(KeyCode::Down)).unwrap();
        app.handle_effort_picker_key(key(KeyCode::Enter)).unwrap(); // Enter ticks while focused
        assert!(app.picker_workflows);
        app.handle_effort_picker_key(key(KeyCode::Right)).unwrap();
        assert_eq!(current(&app), Effort::High);
        assert!(!app.picker_focus_workflows);
        app.handle_effort_picker_key(key(KeyCode::Enter)).unwrap();
        assert!(app.settings.workflows && app.settings.effort == Effort::High);
    }

    #[test]
    fn a_tick_never_leaks_onto_the_higher_levels() {
        use crossterm::event::KeyCode;
        let mut app = picker_app(Some("claude-opus-5-5"), true, Effort::High);
        app.handle_effort_picker_key(key(KeyCode::Down)).unwrap();
        app.handle_effort_picker_key(key(KeyCode::Char(' ')))
            .unwrap();
        app.handle_effort_picker_key(key(KeyCode::Up)).unwrap();
        app.handle_effort_picker_key(key(KeyCode::Right)).unwrap(); // XHigh
        app.handle_effort_picker_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(app.settings.effort, Effort::XHigh);
        assert!(
            !app.settings.workflows,
            "XHigh has its own workflow tier: Super"
        );
        assert!(!app.settings.workflows_active());
    }

    #[test]
    fn down_on_xhigh_and_max_goes_to_super_and_ultimate() {
        use crossterm::event::KeyCode;
        let mut app = picker_app(Some("claude-opus-5-5"), true, Effort::XHigh);
        app.handle_effort_picker_key(key(KeyCode::Down)).unwrap();
        assert_eq!(current(&app), Effort::Super);
        app.handle_effort_picker_key(key(KeyCode::Up)).unwrap();
        assert_eq!(
            current(&app),
            Effort::XHigh,
            "up returns to the plain level"
        );
        app.handle_effort_picker_key(key(KeyCode::Right)).unwrap();
        assert_eq!(current(&app), Effort::Max);
        app.handle_effort_picker_key(key(KeyCode::Down)).unwrap();
        assert_eq!(current(&app), Effort::Ultimate);
        app.handle_effort_picker_key(key(KeyCode::Up)).unwrap();
        assert_eq!(current(&app), Effort::Max);
    }

    #[test]
    fn while_locked_down_explains_instead_of_moving() {
        use crossterm::event::KeyCode;
        let mut app = picker_app(Some("claude-opus-5-5"), false, Effort::XHigh);
        app.handle_effort_picker_key(key(KeyCode::Down)).unwrap();
        assert_eq!(current(&app), Effort::XHigh);
        assert!(app.notice.contains("locked"), "{}", app.notice);
        let mut plain = picker_app(Some("claude-opus-5-5"), false, Effort::High);
        plain.handle_effort_picker_key(key(KeyCode::Down)).unwrap();
        assert!(!plain.picker_focus_workflows, "no checkbox while locked");
        assert!(
            plain.notice.contains("Workflows are locked"),
            "{}",
            plain.notice
        );
        let shown = screen_of(&plain);
        assert!(shown.contains("(locked)"), "{shown}");
        // Entering a locked tier is refused.
        let mut locked = picker_app(Some("claude-opus-5-5"), false, Effort::High);
        locked.picker_index = locked.picker_levels().len() - 2; // Super
        locked
            .handle_effort_picker_key(key(KeyCode::Enter))
            .unwrap();
        assert_eq!(locked.settings.effort, Effort::High);
        assert!(locked.notice.contains("locked"), "{}", locked.notice);
    }

    #[test]
    fn the_workflow_tiers_need_the_level_they_are_built_on() {
        let tiers = |model: &str| {
            picker_app(Some(model), true, Effort::High)
                .picker_levels()
                .into_iter()
                .filter(|level| level.is_workflow_tier())
                .collect::<Vec<_>>()
        };
        assert_eq!(tiers("claude-opus-5-5"), [Effort::Super, Effort::Ultimate]);
        assert_eq!(tiers("gpt-5.5"), [Effort::Super]);
        assert!(tiers("gemini-3-pro").is_empty());
        assert!(tiers("deepseek-chat").is_empty());
        // DeepSeek V4 and Kimi K3 have Max but no XHigh, so only Ultimate.
        assert_eq!(tiers("deepseek-flash"), [Effort::Ultimate]);
        assert_eq!(tiers("kimi-k3"), [Effort::Ultimate]);
    }

    #[test]
    fn a_levels_label_shows_what_it_really_runs_at() {
        use super::mapping_label;
        let gemini = [Effort::Low, Effort::High];
        assert_eq!(mapping_label(Effort::Low, &gemini, true, false), "low");
        assert_eq!(
            mapping_label(Effort::Super, &gemini, true, false),
            "high+wf",
            "capped"
        );
        assert_eq!(
            mapping_label(Effort::Super, &gemini, false, false),
            "locked"
        );
        assert_eq!(mapping_label(Effort::High, &gemini, true, true), "high+wf");
        assert_eq!(
            mapping_label(Effort::XHigh, &gemini, true, true),
            "high",
            "no tick above High"
        );
        assert_eq!(mapping_label(Effort::Max, &[], true, false), "default");
    }

    #[test]
    fn the_flash_fades_smoothly_instead_of_cutting_off() {
        use super::{FLASH_SECONDS, flash_strength};
        use std::time::Duration;
        assert_eq!(flash_strength(Duration::from_secs_f32(FLASH_SECONDS)), 1.0);
        assert_eq!(
            flash_strength(Duration::from_secs_f32(1.2)),
            1.0,
            "full until the fade starts"
        );
        assert_eq!(flash_strength(Duration::ZERO), 0.0);
        // Strictly decreasing through the fade, with no step bigger than a few percent.
        let mut previous = 1.0f32;
        for millisecond in (0..=1200).rev().step_by(10) {
            let strength = flash_strength(Duration::from_millis(millisecond));
            assert!(strength <= previous + 1e-6, "{millisecond}ms");
            assert!(
                previous - strength < 0.05,
                "a jump of {} at {millisecond}ms",
                previous - strength
            );
            previous = strength;
        }
    }

    #[test]
    fn the_faded_name_meets_the_plain_name_at_both_ends_without_a_snap() {
        use super::{effort_color, faded_gradient_name, gradient_name};
        for effort in [Effort::XHigh, Effort::Max, Effort::Super, Effort::Ultimate] {
            let plain = Some(effort_color(effort, 1.0));
            let faded_out = faded_gradient_name(effort, 3, 0.0);
            assert!(
                faded_out.iter().all(|span| span.style.fg == plain),
                "{effort:?}: at strength 0 every letter is the plain color"
            );
            let full = faded_gradient_name(effort, 3, 1.0);
            let shining = gradient_name(effort, true, 3);
            let colors = |spans: &[ratatui::text::Span<'_>]| {
                spans.iter().map(|span| span.style.fg).collect::<Vec<_>>()
            };
            assert_eq!(
                colors(&full),
                colors(&shining),
                "{effort:?}: at strength 1 it is the gradient"
            );
            let half = faded_gradient_name(effort, 3, 0.5);
            assert_ne!(colors(&half), colors(&full));
            assert_ne!(colors(&half), colors(&faded_out));
        }
    }

    fn status_text(
        app: &App,
        now: std::time::Instant,
    ) -> (String, Vec<Option<ratatui::style::Color>>) {
        let spans = super::status_effort_spans(app, 0, now);
        (
            spans.iter().map(|span| span.content.as_ref()).collect(),
            spans.iter().map(|span| span.style.fg).collect(),
        )
    }

    #[test]
    fn the_status_line_name_shines_after_a_change_then_settles_without_a_snap() {
        let mut app = picker_app(Some("claude-opus-5-5"), false, Effort::Max);
        app.apply_effort(Effort::Max).unwrap();
        let start = std::time::Instant::now();
        let settled = status_text(&app, start + std::time::Duration::from_secs(10));
        assert_eq!(settled.0, "max");
        assert!(
            settled
                .1
                .iter()
                .all(|c| *c == Some(super::effort_color(Effort::Max, 1.0)))
        );
        let shining = status_text(&app, start);
        assert_eq!(shining.0, "max");
        let distinct = shining
            .1
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert!(
            distinct > 1,
            "the letters shine in different colors at the start"
        );
        // Partway through the fade the letters are closer to the plain color than at the start.
        let distance = |colors: &[Option<ratatui::style::Color>]| -> u32 {
            let plain = super::effort_color(Effort::Max, 1.0);
            colors
                .iter()
                .map(|c| match (c, plain) {
                    (
                        Some(ratatui::style::Color::Rgb(r, g, b)),
                        ratatui::style::Color::Rgb(pr, pg, pb),
                    ) => {
                        u32::from(r.abs_diff(pr))
                            + u32::from(g.abs_diff(pg))
                            + u32::from(b.abs_diff(pb))
                    }
                    _ => 0,
                })
                .sum()
        };
        let midway = status_text(
            &app,
            app.effort_flash_until.unwrap() - std::time::Duration::from_millis(500),
        );
        assert!(distance(&midway.1) < distance(&shining.1));
        assert!(distance(&midway.1) > 0, "not settled yet");
    }

    #[test]
    fn always_animated_keeps_the_gradient_running() {
        let mut app = picker_app(Some("claude-opus-5-5"), false, Effort::Max);
        app.settings.effort_always_animated = true;
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let (text, colors) = status_text(&app, later);
        assert_eq!(text, "max");
        assert!(
            colors
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1
        );
        let plain_effort = picker_app(Some("claude-opus-5-5"), false, Effort::Low);
        let mut animated = picker_app(Some("claude-opus-5-5"), false, Effort::Low);
        animated.settings.effort_always_animated = true;
        assert_eq!(
            status_text(&plain_effort, later).1,
            status_text(&animated, later).1,
            "levels without a gradient stay plain"
        );
    }

    #[test]
    fn the_status_line_marks_workflows_and_models_without_effort() {
        let mut app = picker_app(Some("claude-opus-5-5"), true, Effort::High);
        app.settings.workflows = true;
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        assert_eq!(status_text(&app, later).0, "high +wf");
        app.settings.workflows = false;
        assert_eq!(status_text(&app, later).0, "high");
        app.settings.workflow_size = crate::workflow::WorkflowSize::Off;
        app.settings.workflows = true;
        assert_eq!(
            status_text(&app, later).0,
            "high",
            "locked workflows are not shown as on"
        );
        let none = picker_app(Some("deepseek-chat"), false, Effort::High);
        assert_eq!(status_text(&none, later).0, "no effort");
        let tier = picker_app(Some("deepseek-chat"), true, Effort::Super);
        assert_eq!(status_text(&tier, later).0, "super");
    }

    #[test]
    fn bar_uses_more_rows_when_space_allows() {
        assert_eq!(bar_rows(30), 4);
        assert_eq!(bar_rows(12), 2);
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
