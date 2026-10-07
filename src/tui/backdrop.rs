use ratatui::layout::Rect;
use ratatui::style::Color;

pub(super) const PARTICLE_GLYPHS: [&str; 4] = ["⋅", "∘", "✧", "✶"];
const FLARE_GLYPH: &str = "✶";
// The aurora fades out before this fraction of the screen height.
const AURORA_REACH: f32 = 0.45;

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

/// A very faint teal shimmer along the top of the screen, drawn only behind empty cells.
fn draw_aurora(buffer: &mut ratatui::buffer::Buffer, area: Rect, t: f32) {
    let reach = (area.height as f32 * AURORA_REACH).floor() as u16;
    for row in 0..reach {
        let falloff = (1.0 - row as f32 / reach as f32).powf(1.5);
        for column in 0..area.width {
            let x = column as f32;
            let wave = 0.5 + 0.5 * (x * 0.09 + t * 0.35 + (x * 0.031 + t * 0.12).sin() * 2.0).sin();
            let strength = wave * falloff;
            if strength < 0.12 {
                continue;
            }
            let greenness = 0.6 + 0.4 * (x * 0.05 + t * 0.2).sin();
            let cell = &mut buffer[(area.x + column, area.y + row)];
            if cell.symbol() == " " {
                cell.set_bg(Color::Rgb(
                    (6.0 + 10.0 * strength) as u8,
                    (16.0 + 34.0 * strength * greenness) as u8,
                    (24.0 + 40.0 * strength) as u8,
                ));
            }
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
    draw_aurora(buffer, area, t);
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
            // Only the nearest layer flares, and only at the peak of its pulse.
            let flare = layer == 2 && pulse > 0.96;
            let shade = if flare {
                0.9
            } else {
                LAYER_BRIGHTNESS[layer] * (0.55 + 0.45 * pulse)
            };
            let channel = |full: f32| (full * shade / 0.45).min(255.0) as u8;
            cell.set_symbol(if flare {
                FLARE_GLYPH
            } else {
                PARTICLE_GLYPHS[layer]
            })
            .set_fg(Color::Rgb(channel(150.0), channel(205.0), channel(235.0)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PARTICLE_GLYPHS, backdrop_enabled, draw_backdrop};
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
    fn the_nearest_particles_sometimes_flare() {
        let flare = (0..400).any(|step| {
            let buffer = buffer_at(step as f32 * 0.25);
            (0..24).any(|y| (0..80).any(|x| buffer[(x, y)].symbol() == "✶"))
        });
        assert!(flare, "no flare in 100 seconds of animation");
    }

    #[test]
    fn a_faint_aurora_shimmers_in_the_upper_part_and_moves() {
        let first = tinted(&buffer_at(0.0));
        assert!(first.len() > 40, "{} tinted cells", first.len());
        assert!(
            first.iter().all(|(_, y)| *y < 14),
            "the lower part of the screen stays clear"
        );
        assert_ne!(first, tinted(&buffer_at(7.0)));
    }

    #[test]
    fn the_aurora_stays_dark_enough_to_read_text_over() {
        let buffer = buffer_at(1.0);
        for (x, y) in tinted(&buffer) {
            let Color::Rgb(r, g, b) = buffer[(x, y)].bg else {
                unreachable!()
            };
            assert!(r < 40 && g < 90 && b < 110, "({x},{y}) is {r},{g},{b}");
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
