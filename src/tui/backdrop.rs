//! The animated backdrop behind the welcome screen (and, if asked, the conversation).
//!
//! Every kind only ever paints cells that are still empty, so text, borders and popups drawn on
//! top of it are never disturbed. `dim` scales how strongly it shows (1.0 is full strength).

use crate::tui::theme::{BackdropKind, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// The glyphs the original (Cool) backdrop draws.
pub(super) const PARTICLE_GLYPHS: [&str; 4] = ["⋅", "∘", "✧", "✶"];
const SPARKLE_GLYPH: &str = "✶";
// One sparkle spot per this many cells.
const CELLS_PER_SPARKLE: usize = 160;
const SPARKLE_SECONDS: f32 = 5.0;
// Share of each cycle a sparkle spends shining; it rests for the remainder.
const SPARKLE_SHINE: f32 = 0.8;
// Chance that a sparkle appears in a given cycle.
const SPARKLE_CHANCE: f32 = 0.7;
// Cells per twinkling star in the star fields.
const CELLS_PER_STAR: usize = 55;

pub(super) fn backdrop_enabled(setting: bool, no_color: bool) -> bool {
    setting && !no_color
}

fn unit(seed: u32) -> f32 {
    let mut h = seed.wrapping_mul(2_654_435_761);
    h = (h ^ (h >> 15)).wrapping_mul(2_246_822_519);
    ((h ^ (h >> 13)) & 0xffff) as f32 / 65_535.0
}

fn wave(phase: f32) -> f32 {
    0.5 + 0.5 * phase.sin()
}

/// `color` at `amount` strength.
fn tone(color: (u8, u8, u8), amount: f32) -> Color {
    let channel = |value: u8| (f32::from(value) * amount).clamp(0.0, 255.0) as u8;
    Color::Rgb(channel(color.0), channel(color.1), channel(color.2))
}

fn mix(from: (u8, u8, u8), to: (u8, u8, u8), amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount) as u8;
    Color::Rgb(
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

/// The theme's screen color as plain channels (black when the terminal's own is used).
fn screen_rgb(theme: &Theme) -> (u8, u8, u8) {
    match theme.screen_bg {
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// Writes `glyph` into an empty cell. Returns whether the cell was free.
fn put(buffer: &mut Buffer, area: Rect, x: u16, y: u16, glyph: &str, color: Color) -> bool {
    if x >= area.width || y >= area.height {
        return false;
    }
    let cell = &mut buffer[(area.x + x, area.y + y)];
    if cell.symbol() != " " {
        return false;
    }
    cell.set_symbol(glyph).set_fg(color);
    true
}

/// Tints the background of an empty cell.
fn tint(buffer: &mut Buffer, area: Rect, x: u16, y: u16, color: Color) {
    if x >= area.width || y >= area.height {
        return;
    }
    let cell = &mut buffer[(area.x + x, area.y + y)];
    if cell.symbol() == " " {
        cell.set_bg(color);
    }
}

pub(super) fn draw_backdrop(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    t: f32,
    theme: &Theme,
    dim: f32,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let buffer = frame.buffer_mut();
    match theme.backdrop {
        BackdropKind::Snow => {
            draw_sparkles(buffer, area, t, dim);
            draw_drift(buffer, area, t, &SNOW, theme, dim);
        }
        BackdropKind::Petals => draw_drift(buffer, area, t, &PETALS, theme, dim),
        BackdropKind::Leaves => draw_drift(buffer, area, t, &LEAVES, theme, dim),
        BackdropKind::Bubbles => draw_drift(buffer, area, t, &BUBBLES, theme, dim),
        BackdropKind::Nebula => {
            draw_nebula(buffer, area, t, theme, dim);
            draw_stars(buffer, area, t, theme, dim);
        }
        BackdropKind::DeepSpace => draw_stars(buffer, area, t, theme, dim),
        BackdropKind::Crt => draw_crt(buffer, area, t, theme, dim),
        BackdropKind::Synthwave => draw_synthwave(buffer, area, t, theme, dim),
    }
}

// ---------------------------------------------------------------------------------------------
// Drifting particles: snow, petals, leaves, bubbles.

/// How a field of drifting particles behaves. Index 0 is the far layer, 2 the near one.
struct Drift {
    glyphs: [&'static [&'static str]; 3],
    /// Rows travelled per second, per layer.
    speed: [f32; 3],
    /// Sideways sway in cells, per layer.
    sway: [f32; 3],
    /// Particles rise instead of fall.
    upward: bool,
    /// Sideways distance covered over one full trip, as a share of the screen height.
    wind: f32,
    /// How strongly slow gusts push the near layers.
    gust: f32,
    /// Each particle picks one of the theme's three colors instead of its layer's.
    varied: bool,
    cells_per_particle: usize,
}

const SNOW: Drift = Drift {
    glyphs: [&["⋅"], &["∘"], &["✧"]],
    speed: [0.25, 0.4, 0.6],
    sway: [0.6, 1.2, 1.8],
    upward: false,
    wind: 0.0,
    gust: 2.5,
    varied: false,
    cells_per_particle: 45,
};

const PETALS: Drift = Drift {
    glyphs: [&["·"], &["❀", "·"], &["✿", "❀"]],
    speed: [0.35, 0.5, 0.7],
    sway: [1.5, 2.6, 3.6],
    upward: false,
    wind: 0.8,
    gust: 1.5,
    varied: true,
    cells_per_particle: 60,
};

const LEAVES: Drift = Drift {
    glyphs: [&["·"], &["❧", "·"], &["❦", "❧"]],
    speed: [0.35, 0.5, 0.7],
    sway: [2.0, 3.2, 4.4],
    upward: false,
    wind: -0.7,
    gust: 2.0,
    varied: true,
    cells_per_particle: 70,
};

const BUBBLES: Drift = Drift {
    glyphs: [&["·"], &["∘"], &["○", "∘"]],
    speed: [0.3, 0.45, 0.65],
    sway: [0.6, 1.0, 1.4],
    upward: true,
    wind: 0.0,
    gust: 0.8,
    varied: false,
    cells_per_particle: 55,
};

/// Where particle `index` is at time `t`, as a cell offset inside a `width` x `height` field.
fn drift_cell(spec: &Drift, index: u32, t: f32, width: f32, height: f32) -> (u16, u16) {
    let layer = (index % 3) as usize;
    let trip = (unit(index * 3 + 1) + t * spec.speed[layer] / height).fract();
    let progress = if spec.upward { 1.0 - trip } else { trip };
    let gust = (t * 0.17).sin() * (t * 0.05).cos();
    let sway = (t * 0.5 + unit(index * 3 + 2) * 6.3).sin() * spec.sway[layer]
        + gust * (layer as f32 + 1.0) * spec.gust
        + spec.wind * trip * height;
    let x = (unit(index * 3) * width + sway).rem_euclid(width);
    let y = (progress * height).min(height - 1.0);
    (x as u16, y as u16)
}

fn draw_drift(buffer: &mut Buffer, area: Rect, t: f32, spec: &Drift, theme: &Theme, dim: f32) {
    let (width, height) = (f32::from(area.width), f32::from(area.height));
    let count = (usize::from(area.width) * usize::from(area.height)) / spec.cells_per_particle;
    for index in 0..count as u32 {
        let layer = (index % 3) as usize;
        let (x, y) = drift_cell(spec, index, t, width, height);
        let choices = spec.glyphs[layer];
        let glyph = choices[(unit(index * 3 + 5) * choices.len() as f32) as usize % choices.len()];
        let base = if spec.varied {
            theme.particles[(unit(index * 3 + 6) * 3.0) as usize % 3]
        } else {
            theme.particles[layer]
        };
        // Particles nearer the viewer shine brighter; each pulses at its own pace.
        let depth = if spec.varied {
            [0.6, 0.8, 1.0][layer]
        } else {
            1.0
        };
        let pulse = 0.55 + 0.45 * twinkle(index, t);
        put(buffer, area, x, y, glyph, tone(base, depth * pulse * dim));
    }
}

/// How bright a particle is right now: each one pulses at its own pace and phase.
fn twinkle(particle: u32, t: f32) -> f32 {
    let pace = 0.8 + unit(particle * 3 + 3) * 1.4;
    let phase = unit(particle * 3 + 4) * 6.3;
    0.7 + 0.3 * (t * pace + phase).sin()
}

/// One sparkle: a fixed spot that fades in, peaks and fades out, then reappears elsewhere.
/// Returns the cell and its brightness (0..=1), or `None` while this sparkle is resting.
fn sparkle(index: u32, t: f32, width: u16, height: u16) -> Option<(u16, u16, f32)> {
    let seed = index * 7919;
    let length = SPARKLE_SECONDS * (0.7 + unit(seed + 1) * 0.8);
    let shifted = t + unit(seed + 2) * length;
    let cycle = (shifted / length).floor();
    let age = shifted / length - cycle;
    // Each cycle is a new place and may be skipped altogether, so sparkles come and go.
    let place = seed.wrapping_add((cycle as i64).rem_euclid(10_007) as u32 * 31);
    if unit(place + 3) > SPARKLE_CHANCE || age >= SPARKLE_SHINE {
        return None;
    }
    let level = (std::f32::consts::PI * age / SPARKLE_SHINE).sin().powi(2);
    let x = (unit(place) * f32::from(width)) as u16;
    let y = (unit(place + 1) * f32::from(height)) as u16;
    Some((x.min(width - 1), y.min(height - 1), level))
}

fn draw_sparkles(buffer: &mut Buffer, area: Rect, t: f32, dim: f32) {
    let count = (usize::from(area.width) * usize::from(area.height)) / CELLS_PER_SPARKLE;
    for index in 0..count as u32 {
        let Some((x, y, level)) = sparkle(index, t, area.width, area.height) else {
            continue;
        };
        let glyph = match level {
            l if l < 0.25 => continue,
            l if l < 0.55 => PARTICLE_GLYPHS[0],
            l if l < 0.85 => PARTICLE_GLYPHS[2],
            _ => SPARKLE_GLYPH,
        };
        let strength = (0.35 + 0.65 * level) * dim;
        put(buffer, area, x, y, glyph, tone((190, 235, 255), strength));
    }
}

// ---------------------------------------------------------------------------------------------
// Star fields: Galaxy and Galaxy (Void).

fn draw_stars(buffer: &mut Buffer, area: Rect, t: f32, theme: &Theme, dim: f32) {
    let (width, height) = (f32::from(area.width), f32::from(area.height));
    let count = (usize::from(area.width) * usize::from(area.height)) / CELLS_PER_STAR;
    for index in 0..count as u32 {
        let x = (unit(index * 5) * width) as u16;
        let y = (unit(index * 5 + 1) * height) as u16;
        let pace = 0.5 + unit(index * 5 + 2) * 1.8;
        let brightness = wave(t * pace + unit(index * 5 + 4) * 6.3);
        if brightness < 0.25 {
            continue;
        }
        let which = (unit(index * 5 + 3) * 3.0) as usize % 3;
        let glyph = match brightness {
            b if b < 0.5 => "·",
            b if b < 0.82 || which != 2 => "✧",
            _ => "✦",
        };
        put(
            buffer,
            area,
            x,
            y,
            glyph,
            tone(theme.particles[which], (0.35 + 0.65 * brightness) * dim),
        );
    }
    draw_shooting_star(buffer, area, t, theme, dim);
}

/// Every so often a star streaks across the upper sky and leaves a fading trail.
fn draw_shooting_star(buffer: &mut Buffer, area: Rect, t: f32, theme: &Theme, dim: f32) {
    const EVERY: f32 = 11.0;
    const STREAK: f32 = 0.18;
    let cycle = (t / EVERY).floor();
    let age = t / EVERY - cycle;
    if age >= STREAK {
        return;
    }
    let seed = (cycle as i64).rem_euclid(100_000) as u32 * 11;
    let (width, height) = (f32::from(area.width), f32::from(area.height));
    let (start_x, start_y) = (unit(seed) * width * 0.6, unit(seed + 1) * height * 0.4);
    let progress = age / STREAK;
    for step in 0..6u16 {
        let behind = f32::from(step) * 0.035;
        let along = progress - behind;
        if along < 0.0 {
            continue;
        }
        let x = start_x + along * width * 0.35;
        let y = start_y + along * height * 0.22;
        let fade = (1.0 - f32::from(step) / 6.0) * (1.0 - progress * 0.4) * dim;
        let glyph = if step == 0 { "✦" } else { "·" };
        put(
            buffer,
            area,
            x as u16,
            y as u16,
            glyph,
            tone(theme.particles[2], fade),
        );
    }
}

/// Smooth value noise for the nebula clouds.
fn noise(x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let ease = |value: f32| value * value * (3.0 - 2.0 * value);
    let corner = |dx: f32, dy: f32| {
        unit(
            ((x0 + dx) as i32 as u32).wrapping_mul(73_856_093)
                ^ ((y0 + dy) as i32 as u32).wrapping_mul(19_349_663),
        )
    };
    let top = corner(0.0, 0.0) + (corner(1.0, 0.0) - corner(0.0, 0.0)) * ease(fx);
    let bottom = corner(0.0, 1.0) + (corner(1.0, 1.0) - corner(0.0, 1.0)) * ease(fx);
    top + (bottom - top) * ease(fy)
}

/// Faint purple clouds behind the stars, drawn into the background of empty cells only.
fn draw_nebula(buffer: &mut Buffer, area: Rect, t: f32, theme: &Theme, dim: f32) {
    let base = screen_rgb(theme);
    let cloud = (64, 30, 120);
    for y in 0..area.height {
        for x in 0..area.width {
            let density = noise(
                f32::from(x) * 0.06 + t * 0.03,
                f32::from(y) * 0.12 - t * 0.02,
            ) * 0.7
                + noise(f32::from(x) * 0.13 - t * 0.02, f32::from(y) * 0.25 + 9.0) * 0.3;
            let strength = ((density - 0.5) * 2.2).clamp(0.0, 1.0) * 0.55 * dim;
            if strength > 0.04 {
                tint(buffer, area, x, y, mix(base, cloud, strength));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Retro (CRT).

fn draw_crt(buffer: &mut Buffer, area: Rect, t: f32, theme: &Theme, dim: f32) {
    let base = screen_rgb(theme);
    // A faint band of light slides down the screen, like a refresh line.
    let band = (t * 0.3).fract() * (f32::from(area.height) + 8.0) - 4.0;
    for y in 0..area.height {
        let distance = (f32::from(y) - band).abs();
        if distance < 3.5 {
            let glow = (1.0 - distance / 3.5) * 0.16 * dim;
            for x in 0..area.width {
                tint(buffer, area, x, y, mix(base, theme.particles[2], glow));
            }
        }
    }
    // Phosphor noise that re-rolls a few times a second.
    let frame = (t * 9.0) as i32;
    for y in 0..area.height {
        for x in 0..area.width {
            let roll = unit(
                (i32::from(x).wrapping_mul(7919)
                    ^ i32::from(y).wrapping_mul(104_729)
                    ^ frame.wrapping_mul(15_485_863)) as u32,
            );
            if roll > 0.992 {
                let strength = (0.3 + 0.7 * unit(u32::from(x) * 31 + u32::from(y))) * dim;
                put(buffer, area, x, y, "·", tone(theme.particles[1], strength));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Synthwave.

fn draw_synthwave(buffer: &mut Buffer, area: Rect, t: f32, theme: &Theme, dim: f32) {
    let (width, height) = (f32::from(area.width), f32::from(area.height));
    let horizon = (height * 0.58).floor();
    let centre = width / 2.0;
    let base = screen_rgb(theme);
    // The sun: a disc sinking behind the horizon, cut by dark stripes toward its bottom.
    let radius = (height * 0.34).max(2.0);
    for y in 0..(horizon as u16) {
        let up = horizon - f32::from(y) - 0.5;
        if up > radius {
            continue;
        }
        let stripe = up < radius * 0.55 && ((up * 1.4 + t * 0.6) as i32) % 3 == 0;
        if stripe {
            continue;
        }
        let half_width = (radius * radius - up * up).sqrt() * 2.0;
        let heat = up / radius;
        let Color::Rgb(r, g, b) = mix((255, 60, 170), (255, 200, 80), heat) else {
            continue;
        };
        for x in 0..area.width {
            if (f32::from(x) - centre).abs() <= half_width {
                tint(buffer, area, x, y, mix(base, (r, g, b), 0.55 * dim));
            }
        }
    }
    // The grid: lines that rush toward the viewer, and rays that fan out from the horizon.
    let line = theme.particles[1];
    let ray = theme.particles[2];
    let floor_rows = height - horizon;
    if floor_rows < 1.0 {
        return;
    }
    let offset = (t * 0.35).fract();
    for k in 0..12u8 {
        let depth = (f32::from(k) + offset) / 12.0;
        // Squared spacing: lines crowd together near the horizon, as in perspective.
        let y = horizon + depth * depth * floor_rows;
        if y >= height {
            continue;
        }
        let strength = (0.35 + 0.65 * depth) * dim;
        for x in 0..area.width {
            put(buffer, area, x, y as u16, "─", tone(line, strength));
        }
    }
    for ray_index in -9i32..=9 {
        for y in (horizon as u16 + 1)..area.height {
            let depth = (f32::from(y) - horizon) / floor_rows;
            let x = centre + ray_index as f32 * 5.5 * depth * (width / 80.0).max(0.6);
            if x < 0.0 || x >= width {
                continue;
            }
            let glyph = match ray_index {
                i if i < 0 => "╱",
                0 => "│",
                _ => "╲",
            };
            put(
                buffer,
                area,
                x as u16,
                y,
                glyph,
                tone(ray, (0.3 + 0.5 * depth) * dim),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BUBBLES, Drift, PARTICLE_GLYPHS, PETALS, backdrop_enabled, draw_backdrop, drift_cell,
        sparkle,
    };
    use crate::ThemeId;
    use crate::tui::theme::{THEMES, Theme, theme_for};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    fn drawn(theme: &Theme, t: f32, dim: f32, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| draw_backdrop(frame, frame.area(), t, theme, dim))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn cool() -> &'static Theme {
        theme_for(ThemeId::Cool)
    }

    fn channels(color: Color) -> u32 {
        match color {
            Color::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b),
            _ => 0,
        }
    }

    /// Cells that now hold a glyph.
    fn glyph_cells(buffer: &Buffer) -> Vec<(u16, u16)> {
        let area = buffer.area;
        (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .filter(|(x, y)| buffer[(*x, *y)].symbol() != " ")
            .collect()
    }

    fn tinted_cells(buffer: &Buffer) -> usize {
        buffer
            .content()
            .iter()
            .filter(|cell| matches!(cell.bg, Color::Rgb(..)))
            .count()
    }

    /// Total strength of everything the backdrop drew.
    fn energy(buffer: &Buffer) -> u32 {
        buffer
            .content()
            .iter()
            .map(|cell| {
                let glyph = if cell.symbol() == " " {
                    0
                } else {
                    channels(cell.fg)
                };
                glyph + channels(cell.bg)
            })
            .sum()
    }

    #[test]
    fn every_theme_draws_a_visible_backdrop() {
        for theme in &THEMES {
            let buffer = drawn(theme, 3.0, 1.0, 100, 30);
            let shown = glyph_cells(&buffer).len() + tinted_cells(&buffer);
            assert!(shown >= 15, "{}: only {shown} cells", theme.name);
        }
    }

    #[test]
    fn every_backdrop_moves_over_time() {
        for theme in &THEMES {
            let first = drawn(theme, 1.0, 1.0, 100, 30);
            let later = drawn(theme, 4.7, 1.0, 100, 30);
            assert_ne!(first, later, "{} is frozen", theme.name);
        }
    }

    #[test]
    fn dimming_makes_every_backdrop_weaker() {
        for theme in &THEMES {
            let full = energy(&drawn(theme, 2.5, 1.0, 100, 30));
            let dimmed = energy(&drawn(theme, 2.5, 0.4, 100, 30));
            assert!(dimmed < full, "{}: {dimmed} vs {full}", theme.name);
        }
    }

    #[test]
    fn a_backdrop_never_covers_text_drawn_before_it() {
        let line = "wallwallwallwallwallwallwallwallwallwallwallwall";
        for theme in &THEMES {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    let wall = vec![line; 24].join(
                        "
",
                    );
                    frame.render_widget(ratatui::widgets::Paragraph::new(wall), area);
                    draw_backdrop(frame, area, 3.0, theme, 1.0);
                })
                .expect("draw");
            let buffer = terminal.backend().buffer();
            for y in 0..24u16 {
                let row = (0..line.chars().count() as u16)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>();
                assert_eq!(row, line, "{} disturbed row {y}", theme.name);
            }
        }
    }

    #[test]
    fn text_drawn_over_a_backdrop_wins_even_between_words() {
        let line = "wall of text wall of text wall of text wall of text wall";
        for theme in &THEMES {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    draw_backdrop(frame, area, 3.0, theme, 1.0);
                    let wall = vec![line; 24].join(
                        "
",
                    );
                    frame.render_widget(ratatui::widgets::Paragraph::new(wall), area);
                })
                .expect("draw");
            let buffer = terminal.backend().buffer();
            for y in 0..24u16 {
                let row = (0..line.chars().count() as u16)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>();
                assert_eq!(row, line, "{} left a particle in row {y}", theme.name);
            }
        }
    }

    #[test]
    fn every_backdrop_survives_tiny_and_empty_areas() {
        for theme in &THEMES {
            for (w, h) in [(1, 1), (2, 1), (3, 2), (5, 40), (40, 3)] {
                let _ = drawn(theme, 7.3, 1.0, w, h);
            }
            let mut terminal = Terminal::new(TestBackend::new(1, 1)).expect("terminal");
            terminal
                .draw(|frame| {
                    draw_backdrop(frame, ratatui::layout::Rect::default(), 1.0, theme, 1.0);
                })
                .expect("draw");
        }
    }

    #[test]
    fn the_cool_backdrop_is_sparse_and_uses_its_own_glyphs() {
        let buffer = drawn(cool(), 0.0, 1.0, 80, 24);
        let count = glyph_cells(&buffer).len();
        assert!((15..=190).contains(&count), "{count} particles");
        assert!(
            glyph_cells(&buffer)
                .iter()
                .all(|(x, y)| PARTICLE_GLYPHS.contains(&buffer[(*x, *y)].symbol()))
        );
        assert_eq!(
            tinted_cells(&buffer),
            0,
            "snow does not tint the background"
        );
    }

    #[test]
    fn particles_twinkle_instead_of_holding_three_fixed_shades() {
        let buffer = drawn(cool(), 3.0, 1.0, 80, 24);
        let mut shades = Vec::new();
        for (x, y) in glyph_cells(&buffer) {
            let fg = buffer[(x, y)].fg;
            if !shades.contains(&fg) {
                shades.push(fg);
            }
        }
        assert!(shades.len() > 6, "{} distinct shades", shades.len());
    }

    #[test]
    fn sparkles_shine_in_place_and_rise_then_fall() {
        // Follow one sparkle for a long time: whenever it shines it stays on one cell, and its
        // brightness climbs to a single peak and falls back.
        let mut runs = 0;
        let mut previous: Option<(u16, u16, f32)> = None;
        let mut peaks = 0;
        let mut rising = true;
        for step in 0..4000 {
            let now = sparkle(3, step as f32 * 0.02, 80, 24);
            match (previous, now) {
                (Some((px, py, pl)), Some((x, y, l))) => {
                    assert_eq!((px, py), (x, y), "a sparkle never moves while it shines");
                    if rising && l < pl - 1e-6 {
                        rising = false;
                        peaks += 1;
                    } else if !rising && l > pl + 1e-6 {
                        panic!("brightness rose again within one shine");
                    }
                }
                (None, Some(_)) => {
                    runs += 1;
                    rising = true;
                }
                _ => {}
            }
            previous = now;
        }
        assert!(runs >= 5, "only {runs} shines in 80 seconds");
        assert!(peaks >= runs - 1, "each shine peaks once: {peaks}/{runs}");
    }

    #[test]
    fn sparkles_relocate_between_shines() {
        let mut places = std::collections::HashSet::new();
        for step in 0..4000 {
            if let Some((x, y, _)) = sparkle(3, step as f32 * 0.02, 80, 24) {
                places.insert((x, y));
            }
        }
        assert!(places.len() >= 4, "{} distinct places", places.len());
    }

    #[test]
    fn the_brightest_sparkles_use_the_star_glyph() {
        let found = (0..400).any(|step| {
            let buffer = drawn(cool(), step as f32 * 0.25, 1.0, 80, 24);
            (0..24).any(|y| (0..80).any(|x| buffer[(x, y)].symbol() == "✶"))
        });
        assert!(found, "no star in 100 seconds of animation");
    }

    /// Counts how often a particle moves up versus down between close frames (ignoring wraps).
    fn vertical_moves(spec: &Drift) -> (u32, u32) {
        let (mut up, mut down) = (0, 0);
        for index in 0..30u32 {
            for step in 0..200 {
                let t = step as f32 * 0.05;
                let (_, before) = drift_cell(spec, index, t, 100.0, 30.0);
                let (_, after) = drift_cell(spec, index, t + 0.05, 100.0, 30.0);
                if before.abs_diff(after) > 3 {
                    continue; // wrapped around
                }
                match after.cmp(&before) {
                    std::cmp::Ordering::Less => up += 1,
                    std::cmp::Ordering::Greater => down += 1,
                    std::cmp::Ordering::Equal => {}
                }
            }
        }
        (up, down)
    }

    #[test]
    fn bubbles_rise_and_petals_fall() {
        let (up, down) = vertical_moves(&BUBBLES);
        assert!(up > 20 && down == 0, "bubbles: up {up}, down {down}");
        let (up, down) = vertical_moves(&PETALS);
        assert!(down > 20 && up == 0, "petals: up {up}, down {down}");
    }

    #[test]
    fn petals_drift_sideways_as_they_fall() {
        let start = i32::from(drift_cell(&PETALS, 4, 0.0, 100.0, 30.0).0);
        let spread = (1..60)
            .map(|step| i32::from(drift_cell(&PETALS, 4, step as f32 * 0.1, 100.0, 30.0).0))
            .map(|x| (x - start).abs())
            .max()
            .unwrap_or(0);
        assert!(spread >= 3, "petals only moved {spread} columns");
    }

    #[test]
    fn the_star_fields_hold_still_while_snow_keeps_moving() {
        let overlap = |theme: &Theme| {
            let a = glyph_cells(&drawn(theme, 3.0, 1.0, 100, 30));
            let b = glyph_cells(&drawn(theme, 3.1, 1.0, 100, 30));
            let same = a.iter().filter(|cell| b.contains(cell)).count();
            same as f32 / a.len().max(1) as f32
        };
        assert!(overlap(theme_for(ThemeId::GalaxyVoid)) > 0.8);
        assert!(overlap(theme_for(ThemeId::Galaxy)) > 0.8);
        assert!(overlap(cool()) < 0.9, "snow keeps drifting");
    }

    #[test]
    fn galaxy_has_a_nebula_and_void_stays_pure_black() {
        let galaxy = drawn(theme_for(ThemeId::Galaxy), 3.0, 1.0, 100, 30);
        assert!(
            tinted_cells(&galaxy) > 100,
            "{} tinted",
            tinted_cells(&galaxy)
        );
        let void = drawn(theme_for(ThemeId::GalaxyVoid), 3.0, 1.0, 100, 30);
        assert_eq!(tinted_cells(&void), 0, "no clouds on the OLED theme");
    }

    #[test]
    fn synthwave_draws_a_sun_a_horizon_and_a_grid() {
        let buffer = drawn(theme_for(ThemeId::Synthwave), 2.0, 1.0, 100, 30);
        assert!(tinted_cells(&buffer) > 150, "the sun");
        let symbols = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect::<String>();
        for part in ["─", "╱", "╲"] {
            assert!(symbols.contains(part), "missing grid piece {part}");
        }
        // The grid belongs to the floor, in the lower part of the screen.
        let highest = (0..30u16)
            .find(|y| (0..100u16).any(|x| buffer[(x, *y)].symbol() == "─"))
            .unwrap_or(0);
        assert!(highest >= 15, "grid starts at row {highest}");
    }

    #[test]
    fn the_crt_backdrop_has_a_scanline_and_noise() {
        let buffer = drawn(theme_for(ThemeId::Retro), 2.0, 1.0, 100, 30);
        assert!(tinted_cells(&buffer) > 40, "the scanline band");
        assert!(!glyph_cells(&buffer).is_empty(), "phosphor noise");
    }

    #[test]
    fn backdrop_respects_setting_and_no_color() {
        assert!(backdrop_enabled(true, false));
        assert!(!backdrop_enabled(false, false));
        assert!(!backdrop_enabled(true, true));
    }
}
