//! Themes: the accent colors, panel colors, logo colors and backdrop style the interface uses.
//!
//! The drawing code asks for the current theme instead of hard-coding colors. The theme follows
//! the settings: `draw` calls `set_current` at the start of every frame, so a change made in
//! Settings is visible on the very next frame.

use crate::ThemeId;
use ratatui::style::Color;
use std::cell::Cell;

/// What the animated backdrop looks like.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BackdropKind {
    /// Ice crystals drifting down.
    Snow,
    /// Twinkling stars that come and go in place, with a faint nebula behind them.
    Nebula,
    /// Twinkling stars only, on pure black.
    DeepSpace,
    /// Petals tumbling down, swaying wide.
    Petals,
    /// Bubbles rising.
    Bubbles,
    /// Leaves falling and turning.
    Leaves,
    /// Flickering phosphor noise and a slow scanline.
    Crt,
    /// A glowing horizon with a perspective grid.
    Synthwave,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Theme {
    pub(super) id: ThemeId,
    pub(super) name: &'static str,
    pub(super) summary: &'static str,
    /// Borders, selection markers and headings.
    pub(super) accent: Color,
    /// Highlighted text and the current row.
    pub(super) accent_bright: Color,
    /// Gentle emphasis: titles, decorations.
    pub(super) accent_soft: Color,
    /// Popups and the settings screen.
    pub(super) panel: Color,
    /// The settings screen and stats, one step darker than a popup.
    pub(super) panel_alt: Color,
    /// Confirmation dialogs.
    pub(super) dialog: Color,
    /// The prompt box.
    pub(super) input: Color,
    /// Painted behind everything; `None` keeps the terminal's own background.
    pub(super) screen_bg: Option<Color>,
    /// Gradient for the first word of the logo (four stops).
    pub(super) logo_primary: [[f32; 3]; 4],
    /// Gradient for the second word of the logo (three stops).
    pub(super) logo_secondary: [[f32; 3]; 3],
    /// The white-hot flash of the launch bloom.
    pub(super) bloom: Color,
    pub(super) tagline: Color,
    pub(super) rule: Color,
    pub(super) backdrop: BackdropKind,
    /// Three particle colors, far to near.
    pub(super) particles: [(u8, u8, u8); 3],
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub(super) const THEMES: [Theme; 8] = [
    Theme {
        id: ThemeId::Cool,
        name: "Cool",
        summary: "Ice blue; snow drifting down. The original look.",
        accent: rgb(98, 213, 244),
        accent_bright: rgb(120, 220, 245),
        accent_soft: rgb(135, 226, 250),
        panel: rgb(25, 32, 38),
        panel_alt: rgb(22, 24, 27),
        dialog: rgb(35, 31, 26),
        input: rgb(37, 38, 40),
        screen_bg: None,
        logo_primary: [
            [83.0, 197.0, 237.0],
            [135.0, 226.0, 250.0],
            [215.0, 249.0, 255.0],
            [130.0, 190.0, 246.0],
        ],
        logo_secondary: [
            [150.0, 162.0, 178.0],
            [226.0, 232.0, 240.0],
            [170.0, 182.0, 198.0],
        ],
        bloom: rgb(232, 251, 255),
        tagline: rgb(128, 158, 184),
        rule: rgb(58, 80, 100),
        backdrop: BackdropKind::Snow,
        particles: [(66, 90, 100), (96, 132, 148), (150, 205, 235)],
    },
    Theme {
        id: ThemeId::Galaxy,
        name: "Galaxy",
        summary: "Deep indigo with violet light; stars twinkling in a faint nebula.",
        accent: rgb(170, 140, 255),
        accent_bright: rgb(200, 175, 255),
        accent_soft: rgb(150, 200, 255),
        panel: rgb(22, 18, 40),
        panel_alt: rgb(18, 14, 34),
        dialog: rgb(18, 14, 34),
        input: rgb(26, 21, 48),
        screen_bg: Some(rgb(14, 10, 30)),
        logo_primary: [
            [120.0, 90.0, 255.0],
            [190.0, 120.0, 255.0],
            [255.0, 150.0, 230.0],
            [110.0, 160.0, 255.0],
        ],
        logo_secondary: [
            [160.0, 170.0, 220.0],
            [235.0, 230.0, 255.0],
            [180.0, 170.0, 230.0],
        ],
        bloom: rgb(245, 235, 255),
        tagline: rgb(150, 140, 200),
        rule: rgb(60, 50, 100),
        backdrop: BackdropKind::Nebula,
        particles: [(110, 100, 170), (170, 150, 235), (235, 225, 255)],
    },
    Theme {
        id: ThemeId::GalaxyVoid,
        name: "Galaxy (Void)",
        summary: "Galaxy on near-OLED black: no nebula, just stars.",
        accent: rgb(180, 160, 255),
        accent_bright: rgb(205, 190, 255),
        accent_soft: rgb(160, 190, 255),
        panel: rgb(5, 5, 9),
        panel_alt: rgb(3, 3, 6),
        dialog: rgb(3, 3, 6),
        input: rgb(10, 10, 16),
        screen_bg: Some(rgb(0, 0, 0)),
        logo_primary: [
            [120.0, 90.0, 255.0],
            [190.0, 120.0, 255.0],
            [255.0, 150.0, 230.0],
            [110.0, 160.0, 255.0],
        ],
        logo_secondary: [
            [160.0, 170.0, 220.0],
            [235.0, 230.0, 255.0],
            [180.0, 170.0, 230.0],
        ],
        bloom: rgb(245, 235, 255),
        tagline: rgb(130, 125, 175),
        rule: rgb(40, 36, 70),
        backdrop: BackdropKind::DeepSpace,
        particles: [(90, 86, 130), (150, 140, 210), (240, 236, 255)],
    },
    Theme {
        id: ThemeId::Sakura,
        name: "Sakura",
        summary: "Soft pink on dark plum; petals tumbling down.",
        accent: rgb(255, 150, 190),
        accent_bright: rgb(255, 180, 210),
        accent_soft: rgb(255, 200, 225),
        panel: rgb(36, 24, 32),
        panel_alt: rgb(30, 19, 27),
        dialog: rgb(30, 19, 27),
        input: rgb(42, 28, 38),
        screen_bg: Some(rgb(26, 16, 24)),
        logo_primary: [
            [255.0, 150.0, 190.0],
            [255.0, 190.0, 215.0],
            [255.0, 225.0, 235.0],
            [240.0, 140.0, 200.0],
        ],
        logo_secondary: [
            [200.0, 170.0, 185.0],
            [250.0, 235.0, 240.0],
            [215.0, 185.0, 200.0],
        ],
        bloom: rgb(255, 240, 245),
        tagline: rgb(190, 150, 170),
        rule: rgb(90, 60, 76),
        backdrop: BackdropKind::Petals,
        particles: [(150, 96, 120), (214, 130, 168), (255, 182, 208)],
    },
    Theme {
        id: ThemeId::Mint,
        name: "Mint",
        summary: "Dark green with neon cyan; bubbles rising.",
        accent: rgb(64, 255, 220),
        accent_bright: rgb(120, 255, 235),
        accent_soft: rgb(150, 255, 240),
        panel: rgb(10, 32, 28),
        panel_alt: rgb(7, 26, 22),
        dialog: rgb(7, 26, 22),
        input: rgb(14, 40, 35),
        screen_bg: Some(rgb(6, 22, 18)),
        logo_primary: [
            [40.0, 230.0, 170.0],
            [80.0, 255.0, 210.0],
            [150.0, 255.0, 235.0],
            [30.0, 200.0, 200.0],
        ],
        logo_secondary: [
            [140.0, 190.0, 170.0],
            [215.0, 245.0, 230.0],
            [160.0, 205.0, 185.0],
        ],
        bloom: rgb(225, 255, 248),
        tagline: rgb(110, 170, 150),
        rule: rgb(30, 70, 60),
        backdrop: BackdropKind::Bubbles,
        particles: [(30, 110, 96), (50, 190, 165), (90, 255, 225)],
    },
    Theme {
        id: ThemeId::Autumn,
        name: "Autumn",
        summary: "Amber and rust on warm brown; leaves falling.",
        accent: rgb(240, 150, 60),
        accent_bright: rgb(255, 185, 90),
        accent_soft: rgb(255, 205, 130),
        panel: rgb(38, 26, 18),
        panel_alt: rgb(32, 21, 14),
        dialog: rgb(32, 21, 14),
        input: rgb(46, 32, 22),
        screen_bg: Some(rgb(28, 18, 12)),
        logo_primary: [
            [230.0, 110.0, 40.0],
            [255.0, 160.0, 60.0],
            [255.0, 205.0, 110.0],
            [200.0, 80.0, 40.0],
        ],
        logo_secondary: [
            [190.0, 160.0, 130.0],
            [245.0, 225.0, 200.0],
            [205.0, 175.0, 145.0],
        ],
        bloom: rgb(255, 240, 215),
        tagline: rgb(190, 150, 110),
        rule: rgb(90, 60, 40),
        backdrop: BackdropKind::Leaves,
        particles: [(150, 80, 40), (205, 110, 45), (240, 170, 60)],
    },
    Theme {
        id: ThemeId::Retro,
        name: "Retro (CRT)",
        summary: "Amber phosphor on black; flickering noise and a slow scanline.",
        accent: rgb(255, 176, 0),
        accent_bright: rgb(255, 200, 70),
        accent_soft: rgb(255, 215, 120),
        panel: rgb(14, 10, 0),
        panel_alt: rgb(10, 7, 0),
        dialog: rgb(10, 7, 0),
        input: rgb(20, 14, 0),
        screen_bg: Some(rgb(6, 4, 0)),
        logo_primary: [
            [255.0, 160.0, 0.0],
            [255.0, 200.0, 60.0],
            [255.0, 225.0, 130.0],
            [230.0, 140.0, 0.0],
        ],
        logo_secondary: [
            [190.0, 140.0, 40.0],
            [255.0, 215.0, 120.0],
            [215.0, 160.0, 60.0],
        ],
        bloom: rgb(255, 245, 200),
        tagline: rgb(200, 150, 40),
        rule: rgb(90, 60, 0),
        backdrop: BackdropKind::Crt,
        particles: [(90, 62, 0), (150, 104, 0), (230, 160, 0)],
    },
    Theme {
        id: ThemeId::Synthwave,
        name: "Synthwave",
        summary: "Magenta and cyan on deep purple; a glowing horizon and grid.",
        accent: rgb(255, 70, 200),
        accent_bright: rgb(255, 120, 220),
        accent_soft: rgb(90, 230, 255),
        panel: rgb(30, 12, 48),
        panel_alt: rgb(24, 9, 40),
        dialog: rgb(24, 9, 40),
        input: rgb(38, 16, 60),
        screen_bg: Some(rgb(20, 8, 36)),
        logo_primary: [
            [255.0, 60.0, 190.0],
            [255.0, 130.0, 90.0],
            [255.0, 210.0, 80.0],
            [120.0, 90.0, 255.0],
        ],
        logo_secondary: [
            [90.0, 220.0, 255.0],
            [200.0, 245.0, 255.0],
            [60.0, 170.0, 255.0],
        ],
        bloom: rgb(255, 235, 250),
        tagline: rgb(160, 120, 200),
        rule: rgb(80, 40, 120),
        backdrop: BackdropKind::Synthwave,
        particles: [(120, 40, 150), (255, 70, 200), (90, 230, 255)],
    },
];

pub(super) fn theme_for(id: ThemeId) -> &'static Theme {
    THEMES
        .iter()
        .find(|theme| theme.id == id)
        .unwrap_or(&THEMES[0])
}

thread_local! {
    static CURRENT: Cell<ThemeId> = const { Cell::new(ThemeId::Cool) };
}

/// Chooses the theme every drawing helper on this thread uses from now on.
pub(super) fn set_current(id: ThemeId) {
    CURRENT.with(|current| current.set(id));
}

/// The colors of a theme as the sign-in page needs them.
pub(super) fn login_palette(id: ThemeId) -> crate::login_page::Palette {
    let theme = theme_for(id);
    let channels = |color: Color, fallback: [u8; 3]| match color {
        Color::Rgb(red, green, blue) => [red, green, blue],
        _ => fallback,
    };
    let defaults = crate::login_page::Palette::default();
    let window = channels(theme.panel, defaults.window);
    crate::login_page::Palette {
        background: theme.screen_bg.map_or_else(
            || channels(theme.panel_alt, defaults.background),
            |color| channels(color, defaults.background),
        ),
        window,
        text: defaults.text,
        dim: channels(theme.tagline, defaults.dim),
        accent: channels(theme.accent, defaults.accent),
        good: channels(theme.accent_bright, defaults.good),
        bad: defaults.bad,
    }
}

/// Light mode: every frame is drawn as usual and then repainted with each color's lightness
/// turned around, keeping its hue. Dark panels become pale, light text becomes dark, and accent
/// colors keep their character while staying readable on a light background.
pub(super) fn to_light(buffer: &mut ratatui::buffer::Buffer) {
    for cell in buffer.content.iter_mut() {
        cell.fg = light_foreground(cell.fg);
        cell.bg = light_background(cell.bg);
    }
}

/// The page color of light mode, used where the terminal's own background would show.
const LIGHT_PAGE: Color = rgb(246, 247, 249);

pub(super) fn light_background(color: Color) -> Color {
    match color {
        Color::Reset | Color::Black => LIGHT_PAGE,
        Color::Rgb(r, g, b) => flip_lightness((r, g, b), 0.80, 0.97),
        Color::White | Color::Gray => rgb(70, 72, 76),
        Color::DarkGray => rgb(205, 208, 214),
        other => other,
    }
}

pub(super) fn light_foreground(color: Color) -> Color {
    match color {
        Color::Reset | Color::White => rgb(28, 30, 34),
        Color::Gray => rgb(70, 74, 80),
        Color::DarkGray => rgb(120, 124, 132),
        Color::Black => rgb(246, 247, 249),
        Color::Red | Color::LightRed => rgb(190, 30, 40),
        Color::Green | Color::LightGreen => rgb(20, 125, 55),
        Color::Yellow | Color::LightYellow => rgb(150, 105, 0),
        Color::Blue | Color::LightBlue => rgb(30, 80, 190),
        Color::Magenta | Color::LightMagenta => rgb(150, 40, 150),
        Color::Cyan | Color::LightCyan => rgb(0, 115, 140),
        Color::Rgb(r, g, b) => flip_lightness((r, g, b), 0.12, 0.42),
        other => other,
    }
}

/// Turns the lightness of a color around (dark to light and back), keeping its hue and
/// saturation, and keeps the result between `low` and `high`.
fn flip_lightness((r, g, b): (u8, u8, u8), low: f32, high: f32) -> Color {
    let [r, g, b] = [r, g, b].map(|channel| f32::from(channel) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    let (hue, saturation) = if delta == 0.0 {
        (0.0, 0.0)
    } else {
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        (hue * 60.0, saturation)
    };
    let lightness = (1.0 - lightness).clamp(low, high);
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hue {
        h if h < 60.0 => (chroma, x, 0.0),
        h if h < 120.0 => (x, chroma, 0.0),
        h if h < 180.0 => (0.0, chroma, x),
        h if h < 240.0 => (0.0, x, chroma),
        h if h < 300.0 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let offset = lightness - chroma / 2.0;
    let channel = |value: f32| ((value + offset) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(channel(r), channel(g), channel(b))
}

pub(super) fn current() -> &'static Theme {
    theme_for(CURRENT.with(Cell::get))
}

pub(super) fn accent() -> Color {
    current().accent
}

pub(super) fn accent_bright() -> Color {
    current().accent_bright
}

pub(super) fn accent_soft() -> Color {
    current().accent_soft
}

pub(super) fn panel() -> Color {
    current().panel
}

pub(super) fn panel_alt() -> Color {
    current().panel_alt
}

pub(super) fn dialog() -> Color {
    current().dialog
}

pub(super) fn input() -> Color {
    current().input
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(color: Color) -> (u8, u8, u8) {
        match color {
            Color::Rgb(r, g, b) => (r, g, b),
            other => panic!("expected RGB, got {other:?}"),
        }
    }

    #[test]
    fn the_sign_in_page_takes_its_colors_from_the_theme() {
        let cool = login_palette(ThemeId::Cool);
        let galaxy = login_palette(ThemeId::GalaxyVoid);
        assert_eq!(cool.accent, [98, 213, 244]);
        assert_eq!(
            galaxy.background,
            [0, 0, 0],
            "a theme's own screen color is used"
        );
        assert_eq!(galaxy.window, [5, 5, 9]);
        assert_ne!(cool, galaxy);
        for id in [
            ThemeId::Cool,
            ThemeId::Sakura,
            ThemeId::Synthwave,
            ThemeId::Retro,
        ] {
            assert_ne!(login_palette(id).window, login_palette(id).text, "readable");
        }
    }

    fn lightness(color: Color) -> f32 {
        let (r, g, b) = channels(color);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        (f32::from(max) + f32::from(min)) / 510.0
    }

    #[test]
    fn light_mode_turns_dark_backgrounds_light_and_light_text_dark() {
        for theme in &THEMES {
            for background in [theme.panel, theme.panel_alt, theme.dialog, theme.input] {
                assert!(
                    lightness(light_background(background)) >= 0.8,
                    "{} {background:?}",
                    theme.name
                );
            }
            for text in [
                theme.accent,
                theme.accent_bright,
                theme.accent_soft,
                theme.tagline,
            ] {
                assert!(
                    lightness(light_foreground(text)) <= 0.45,
                    "{} {text:?} would be hard to read on white",
                    theme.name
                );
            }
        }
        assert!(
            lightness(light_background(Color::Reset)) >= 0.9,
            "the terminal's own background"
        );
        for text in [Color::Reset, Color::White, Color::Gray] {
            assert!(lightness(light_foreground(text)) <= 0.35, "{text:?}");
        }
        assert!(lightness(light_foreground(Color::DarkGray)) <= 0.6);
    }

    #[test]
    fn light_mode_keeps_each_colors_hue() {
        // Cool's ice blue stays blue, Sakura's pink stays pink.
        let (r, g, b) = channels(light_foreground(rgb(98, 213, 244)));
        assert!(b > r && g > r, "still blue: {r},{g},{b}");
        let (r, g, b) = channels(light_foreground(rgb(255, 150, 200)));
        assert!(r > g && r > b, "still pink: {r},{g},{b}");
    }

    #[test]
    fn light_mode_repaints_every_cell_of_a_frame() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 1));
        buffer[(0, 0)].set_fg(Color::White).set_bg(rgb(25, 32, 38));
        buffer[(1, 0)].set_fg(rgb(98, 213, 244));
        to_light(&mut buffer);
        assert!(lightness(buffer[(0, 0)].bg) >= 0.8);
        assert!(lightness(buffer[(0, 0)].fg) <= 0.35);
        assert!(
            lightness(buffer[(1, 0)].bg) >= 0.9,
            "unpainted cells get a light background"
        );
        assert!(lightness(buffer[(2, 0)].fg) <= 0.35);
    }

    #[test]
    fn every_theme_is_listed_once_and_findable() {
        let mut names = std::collections::HashSet::new();
        for theme in &THEMES {
            assert!(names.insert(theme.name), "{} twice", theme.name);
            assert_eq!(theme_for(theme.id).name, theme.name);
        }
        assert_eq!(THEMES.len(), 8);
        assert_eq!(THEMES[0].id, ThemeId::Cool, "the original look comes first");
    }

    #[test]
    fn themes_differ_in_accent_and_backdrop() {
        let accents = THEMES
            .iter()
            .map(|theme| channels(theme.accent))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            accents.len(),
            THEMES.len(),
            "every theme has its own accent"
        );
        let kinds = THEMES
            .iter()
            .map(|theme| format!("{:?}", theme.backdrop))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            kinds.len(),
            THEMES.len(),
            "every theme has its own backdrop"
        );
    }

    #[test]
    fn the_original_theme_keeps_the_terminals_own_background() {
        assert!(THEMES[0].screen_bg.is_none());
        assert!(THEMES[1..].iter().all(|theme| theme.screen_bg.is_some()));
    }

    #[test]
    fn void_is_black_enough_for_an_oled_screen() {
        let void = theme_for(ThemeId::GalaxyVoid);
        assert_eq!(channels(void.screen_bg.unwrap()), (0, 0, 0));
        let (r, g, b) = channels(void.panel);
        assert!(r < 10 && g < 10 && b < 14, "panel {r},{g},{b}");
        let galaxy = theme_for(ThemeId::Galaxy);
        let lift = |color: Color| {
            let (r, g, b) = channels(color);
            u32::from(r) + u32::from(g) + u32::from(b)
        };
        assert!(
            lift(galaxy.screen_bg.unwrap()) > 30,
            "Galaxy is visibly lighter"
        );
    }

    #[test]
    fn panels_stay_dark_enough_for_light_text() {
        for theme in &THEMES {
            for color in [theme.panel, theme.panel_alt]
                .into_iter()
                .chain(theme.screen_bg)
            {
                let (r, g, b) = channels(color);
                assert!(
                    u32::from(r) + u32::from(g) + u32::from(b) < 200,
                    "{} has a light surface {r},{g},{b}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn the_current_theme_follows_set_current_and_defaults_to_cool() {
        set_current(ThemeId::Cool);
        assert_eq!(current().id, ThemeId::Cool);
        assert_eq!(accent(), Color::Rgb(98, 213, 244));
        set_current(ThemeId::Sakura);
        assert_eq!(current().id, ThemeId::Sakura);
        assert_eq!(accent(), Color::Rgb(255, 150, 190));
        set_current(ThemeId::Cool);
    }

    #[test]
    fn theme_ids_are_saved_in_kebab_case() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Holder {
            theme: ThemeId,
        }
        let text = toml::to_string(&Holder {
            theme: ThemeId::GalaxyVoid,
        })
        .expect("write");
        assert!(text.contains("galaxy-void"), "{text}");
        let back: Holder = toml::from_str("theme = \"retro\"").expect("read");
        assert_eq!(back.theme, ThemeId::Retro);
        assert!(toml::from_str::<Holder>("theme = \"nope\"").is_err());
    }
}

#[cfg(test)]
mod render_tests {
    use super::THEMES;
    use crate::tui::render::draw;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{Settings, ThemeId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn frame(theme: ThemeId, settings_open: bool) -> ratatui::buffer::Buffer {
        let mut settings = Settings::default();
        settings.theme = theme;
        settings.background_animation = false;
        let mut app = App::new(settings);
        app.trust_prompt = false;
        if settings_open {
            app.open_settings(Section::General);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|f| draw(f, &app, 0)).expect("draw");
        terminal.backend().buffer().clone()
    }

    fn any_cell(
        buffer: &ratatui::buffer::Buffer,
        test: impl Fn(&ratatui::buffer::Cell) -> bool,
    ) -> bool {
        buffer.content().iter().any(test)
    }

    #[test]
    fn each_theme_paints_its_own_accent_on_the_settings_screen() {
        for theme in &THEMES {
            let buffer = frame(theme.id, true);
            assert!(
                any_cell(&buffer, |cell| cell.fg == theme.accent),
                "{} accent missing",
                theme.name
            );
            for other in THEMES.iter().filter(|other| other.id != theme.id) {
                assert!(
                    !any_cell(&buffer, |cell| cell.fg == other.accent),
                    "{} leaked the {} accent",
                    theme.name,
                    other.name
                );
            }
        }
    }

    #[test]
    fn themes_paint_the_whole_screen_except_the_original() {
        let original = frame(ThemeId::Cool, false);
        assert_eq!(
            original.content()[0].bg,
            Color::Reset,
            "the terminal's own background shows through"
        );
        for theme in &THEMES[1..] {
            let buffer = frame(theme.id, false);
            let background = theme.screen_bg.expect("themed background");
            assert!(
                buffer
                    .content()
                    .iter()
                    .all(|cell| cell.bg == background || cell.bg == theme.input),
                "{} leaves unpainted cells",
                theme.name
            );
        }
    }

    #[test]
    fn switching_the_setting_switches_the_next_frame() {
        let first = frame(ThemeId::Mint, false);
        let second = frame(ThemeId::Cool, false);
        assert_ne!(first.content()[0].bg, second.content()[0].bg);
    }
}
