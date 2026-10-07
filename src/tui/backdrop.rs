use ratatui::layout::Rect;
use ratatui::style::Color;

pub(super) const PARTICLE_GLYPHS: [&str; 4] = ["⋅", "∘", "✧", "✶"];
const SPARKLE_GLYPH: &str = "✶";
// One sparkle spot per this many cells.
const CELLS_PER_SPARKLE: usize = 160;
const SPARKLE_SECONDS: f32 = 5.0;
// Share of each cycle a sparkle spends shining; it rests for the remainder.
const SPARKLE_SHINE: f32 = 0.8;
// Chance that a sparkle appears in a given cycle.
const SPARKLE_CHANCE: f32 = 0.7;

// One particle per this many cells keeps the field sparse enough to stay in the background.
const CELLS_PER_PARTICLE: usize = 45;
const LAYER_BRIGHTNESS: [f32; 3] = [0.22, 0.32, 0.45];
const LAYER_SPEED: [f32; 3] = [0.25, 0.4, 0.6];

pub(super) fn backdrop_enabled(setting: bool, no_color: bool) -> bool {
    setting && !no_color
}

fn unit(seed: u32) -> f32 {
    let mut h = seed.wrapping_mul(2_654_435_761);
    h = (h ^ (h >> 15)).wrapping_mul(2_246_822_519);
    ((h ^ (h >> 13)) & 0xffff) as f32 / 65_535.0
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
    let x = (unit(place) * width as f32) as u16;
    let y = (unit(place + 1) * height as f32) as u16;
    Some((x.min(width - 1), y.min(height - 1), level))
}

fn draw_sparkles(buffer: &mut ratatui::buffer::Buffer, area: Rect, t: f32) {
    let count = (area.width as usize * area.height as usize) / CELLS_PER_SPARKLE;
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
        let cell = &mut buffer[(area.x + x, area.y + y)];
        if cell.symbol() == " " {
            let channel = |full: f32| (full * (0.35 + 0.65 * level)).min(255.0) as u8;
            cell.set_symbol(glyph).set_fg(Color::Rgb(
                channel(190.0),
                channel(235.0),
                channel(255.0),
            ));
        }
    }
}

pub(super) fn draw_backdrop(frame: &mut ratatui::Frame<'_>, area: Rect, t: f32) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (width, height) = (area.width as f32, area.height as f32);
    let count = (area.width as usize * area.height as usize) / CELLS_PER_PARTICLE;
    let buffer = frame.buffer_mut();
    draw_sparkles(buffer, area, t);
    // A slow wind that comes and goes, felt more by the particles nearer the viewer.
    let gust = (t * 0.17).sin() * (t * 0.05).cos();
    for particle in 0..count as u32 {
        let layer = (particle % 3) as usize;
        let fall = (unit(particle * 3 + 1) + t * LAYER_SPEED[layer] / height).fract();
        let sway = (t * 0.5 + unit(particle * 3 + 2) * 6.3).sin() * (layer as f32 + 1.0) * 0.6
            + gust * (layer as f32 + 1.0) * 2.5;
        let x = (unit(particle * 3) * width + sway).rem_euclid(width);
        let position = (area.x + x as u16, area.y + (fall * height) as u16);
        let cell = &mut buffer[position];
        if cell.symbol() == " " {
            let pulse = twinkle(particle, t);
            let shade = LAYER_BRIGHTNESS[layer] * (0.55 + 0.45 * pulse);
            let channel = |full: f32| (full * shade / 0.45).min(255.0) as u8;
            cell.set_symbol(PARTICLE_GLYPHS[layer]).set_fg(Color::Rgb(
                channel(150.0),
                channel(205.0),
                channel(235.0),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PARTICLE_GLYPHS, backdrop_enabled, draw_backdrop, sparkle};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn buffer_at(t: f32) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| draw_backdrop(frame, frame.area(), t))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn tinted(buffer: &ratatui::buffer::Buffer) -> Vec<(u16, u16)> {
        let mut cells = Vec::new();
        for y in 0..24 {
            for x in 0..80 {
                if matches!(buffer[(x, y)].bg, Color::Rgb(..)) {
                    cells.push((x, y));
                }
            }
        }
        cells
    }

    fn particles(t: f32) -> Vec<(u16, u16)> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| draw_backdrop(frame, frame.area(), t))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut cells = Vec::new();
        for y in 0..24 {
            for x in 0..80 {
                if PARTICLE_GLYPHS.contains(&buffer[(x, y)].symbol()) {
                    cells.push((x, y));
                }
            }
        }
        cells
    }

    #[test]
    fn backdrop_is_sparse() {
        let count = particles(0.0).len();
        assert!((15..=190).contains(&count), "{count} particles");
    }

    #[test]
    fn particles_twinkle_instead_of_holding_three_fixed_shades() {
        let buffer = buffer_at(3.0);
        let mut shades = Vec::new();
        for y in 0..24 {
            for x in 0..80 {
                let cell = &buffer[(x, y)];
                if PARTICLE_GLYPHS.contains(&cell.symbol()) && !shades.contains(&cell.fg) {
                    shades.push(cell.fg);
                }
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
            let buffer = buffer_at(step as f32 * 0.25);
            (0..24).any(|y| (0..80).any(|x| buffer[(x, y)].symbol() == "✶"))
        });
        assert!(found, "no star in 100 seconds of animation");
    }

    #[test]
    fn nothing_tints_the_background() {
        for t in [0.0, 3.0, 9.0] {
            assert!(tinted(&buffer_at(t)).is_empty());
        }
    }

    #[test]
    fn existing_text_keeps_its_cell() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(ratatui::widgets::Paragraph::new("keep"), frame.area());
                draw_backdrop(frame, frame.area(), 2.0);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let word = (0..4).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        assert_eq!(word, "keep");
        assert_eq!(buffer[(0, 0)].bg, Color::Reset, "text cells are not tinted");
    }

    #[test]
    fn backdrop_drifts_over_time() {
        assert_ne!(particles(0.0), particles(2.0));
    }

    #[test]
    fn backdrop_respects_setting_and_no_color() {
        assert!(backdrop_enabled(true, false));
        assert!(!backdrop_enabled(false, false));
        assert!(!backdrop_enabled(true, true));
    }

    #[test]
    fn backdrop_handles_empty_areas() {
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).expect("terminal");
        terminal
            .draw(|frame| draw_backdrop(frame, ratatui::layout::Rect::default(), 1.0))
            .expect("draw");
    }
}
