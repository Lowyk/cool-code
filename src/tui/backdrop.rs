use ratatui::layout::Rect;
use ratatui::style::Color;

pub(super) const PARTICLE_GLYPHS: [&str; 3] = ["⋅", "∘", "✧"];

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

pub(super) fn draw_backdrop(frame: &mut ratatui::Frame<'_>, area: Rect, t: f32) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (width, height) = (area.width as f32, area.height as f32);
    let count = (area.width as usize * area.height as usize) / CELLS_PER_PARTICLE;
    let buffer = frame.buffer_mut();
    for particle in 0..count as u32 {
        let layer = (particle % 3) as usize;
        let fall = (unit(particle * 3 + 1) + t * LAYER_SPEED[layer] / height).fract();
        let sway = (t * 0.5 + unit(particle * 3 + 2) * 6.3).sin() * (layer as f32 + 1.0) * 0.6;
        let x = (unit(particle * 3) * width + sway).rem_euclid(width);
        let position = (area.x + x as u16, area.y + (fall * height) as u16);
        let cell = &mut buffer[position];
        if cell.symbol() == " " {
            let shade = LAYER_BRIGHTNESS[layer];
            cell.set_symbol(PARTICLE_GLYPHS[layer]).set_fg(Color::Rgb(
                (150.0 * shade / 0.45) as u8,
                (205.0 * shade / 0.45) as u8,
                (235.0 * shade / 0.45) as u8,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PARTICLE_GLYPHS, backdrop_enabled, draw_backdrop};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
