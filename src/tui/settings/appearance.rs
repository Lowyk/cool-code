//! Settings > Appearance: the theme, light mode, the animated backdrop and the effort name, each
//! with a live preview of what it looks like.

use crate::Effort;
use crate::tui::backdrop::draw_backdrop;
use crate::tui::effort::{effort_color, effort_name, faded_gradient_name};
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::tui::theme::{self, THEMES, Theme};
use crate::write_settings;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// One row per theme, then light mode, the three backdrop switches and the effort-name switch.
pub(super) const ROWS: usize = THEMES.len() + 5;
const LIGHT_MODE: usize = THEMES.len();
const WELCOME_BACKDROP: usize = THEMES.len() + 1;
const CHAT_BACKDROP: usize = THEMES.len() + 2;
const DIM_IN_CHAT: usize = THEMES.len() + 3;
const ANIMATE_EFFORT: usize = THEMES.len() + 4;

/// Width of the option list; the preview takes the rest when there is room for it.
const LIST_WIDTH: u16 = 40;
const MIN_PREVIEW_WIDTH: u16 = 30;
/// How strongly the backdrop shows behind a conversation when dimming is on.
const CHAT_DIM: f32 = 0.4;
/// The levels the effort-name preview shows.
const PREVIEW_LEVELS: [Effort; 4] = [Effort::XHigh, Effort::Max, Effort::Super, Effort::Ultimate];

fn switch(on: bool) -> Span<'static> {
    if on {
        Span::styled("on", Style::default().fg(Color::Rgb(110, 220, 130)))
    } else {
        Span::styled("off", Style::default().fg(Color::Gray))
    }
}

/// The effort names as the status line would show them: shining and moving when animated,
/// plain when not.
pub(super) fn effort_preview(animated: bool, tick: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, effort) in PREVIEW_LEVELS.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        if animated {
            spans.extend(faded_gradient_name(*effort, tick, 1.0));
        } else {
            spans.push(Span::styled(
                effort_name(*effort),
                Style::default().fg(effort_color(*effort, 1.0)),
            ));
        }
    }
    spans
}

pub(super) fn draw_appearance(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let elapsed = app.launched_at.elapsed();
    let tick = (elapsed.as_millis() / 280) as usize;
    let with_preview = area.width >= LIST_WIDTH + 2 + MIN_PREVIEW_WIDTH;
    let list_area = Rect {
        width: if with_preview { LIST_WIDTH } else { area.width },
        ..area
    };
    frame.render_widget(
        Paragraph::new(option_lines(app, view, tick, with_preview)),
        list_area,
    );
    if with_preview {
        let preview = Rect::new(
            area.x + LIST_WIDTH + 2,
            area.y,
            area.width - LIST_WIDTH - 2,
            area.height.min(20),
        );
        draw_preview(frame, preview, app, view.row, elapsed.as_secs_f32(), tick);
    }
}

fn option_lines(
    app: &App,
    view: &SettingsView,
    tick: usize,
    with_preview: bool,
) -> Vec<Line<'static>> {
    let focused = view.focus == Focus::Content;
    let current = |index: usize| focused && index == view.row;
    let marker = |index: usize| {
        Span::styled(
            if current(index) { "▸ " } else { "  " },
            Style::default().fg(theme::accent()),
        )
    };
    let label_style = |index: usize| {
        if current(index) {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        }
    };
    let heading = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let settings = &app.settings;
    let mut lines = vec![heading("Theme")];
    for (index, entry) in THEMES.iter().enumerate() {
        let chosen = entry.id == settings.theme;
        let mut spans = vec![
            marker(index),
            Span::styled(
                if chosen { "● " } else { "  " },
                Style::default().fg(Color::Rgb(110, 220, 130)),
            ),
            Span::styled(format!("{:<15}", entry.name), label_style(index)),
            Span::styled("■", Style::default().fg(entry.accent)),
        ];
        // Without room for the preview, the summary is the only description.
        if current(index) && !with_preview {
            spans.push(Span::styled(
                format!("  {}", entry.summary),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans));
    }
    let toggle = |index: usize, label: &str, value: bool| {
        Line::from(vec![
            marker(index),
            Span::raw("  "),
            Span::styled(format!("{label:<21}"), label_style(index)),
            switch(value),
        ])
    };
    lines.push(toggle(LIGHT_MODE, "Light mode", settings.light_mode));
    lines.push(Line::from(""));
    lines.push(heading("Backdrop"));
    lines.push(toggle(
        WELCOME_BACKDROP,
        "Welcome screen",
        settings.background_animation,
    ));
    lines.push(toggle(
        CHAT_BACKDROP,
        "While chatting",
        settings.backdrop_in_chat,
    ));
    lines.push(toggle(
        DIM_IN_CHAT,
        "Dim while chatting",
        settings.dim_backdrop_in_chat,
    ));
    lines.push(Line::from(""));
    lines.push(heading("Effort"));
    lines.push(toggle(
        ANIMATE_EFFORT,
        "Animate effort name",
        settings.effort_always_animated,
    ));
    let mut preview = vec![Span::raw("      ")];
    preview.extend(effort_preview(settings.effort_always_animated, tick));
    lines.push(Line::from(preview));
    lines
}

fn draw_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    row: usize,
    t: f32,
    tick: usize,
) {
    let settings = &app.settings;
    let current = theme::theme_for(settings.theme);
    let chat_dim = if settings.dim_backdrop_in_chat {
        CHAT_DIM
    } else {
        1.0
    };
    let (shown, backdrop, chat, caption): (&Theme, Option<f32>, bool, String) = match row {
        row if row < THEMES.len() => {
            let entry = &THEMES[row];
            (
                entry,
                Some(1.0),
                false,
                format!("{}: {}", entry.name, entry.summary),
            )
        }
        LIGHT_MODE => (
            current,
            None,
            true,
            if settings.light_mode {
                "Light mode is on: the whole interface is drawn light, in your theme's colors."
            } else {
                "Light mode is off: the interface is drawn dark."
            }
            .to_owned(),
        ),
        WELCOME_BACKDROP => (
            current,
            settings.background_animation.then_some(1.0),
            false,
            if settings.background_animation {
                "The welcome screen shows the animated backdrop."
            } else {
                "The welcome screen stays plain."
            }
            .to_owned(),
        ),
        CHAT_BACKDROP => (
            current,
            settings.backdrop_in_chat.then_some(chat_dim),
            true,
            if settings.backdrop_in_chat {
                "The backdrop stays behind the conversation."
            } else {
                "Conversations are drawn on a plain background."
            }
            .to_owned(),
        ),
        DIM_IN_CHAT => (
            current,
            Some(chat_dim),
            true,
            if settings.dim_backdrop_in_chat {
                "Behind a conversation the backdrop is dimmed so text stays easy to read."
            } else {
                "Behind a conversation the backdrop shows at full strength."
            }
            .to_owned(),
        ),
        _ => {
            draw_effort_preview(frame, area, settings.effort_always_animated, tick);
            return;
        }
    };
    let caption_height = 3.min(area.height);
    draw_sample(
        frame,
        Rect {
            height: area.height - caption_height,
            ..area
        },
        shown,
        backdrop,
        chat,
        t,
    );
    frame.render_widget(
        Paragraph::new(caption)
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true }),
        Rect::new(
            area.x,
            area.y + area.height - caption_height,
            area.width,
            caption_height,
        ),
    );
}

/// A small window drawn entirely in \`shown\`'s colors: its backdrop, then either the welcome
/// title or a short conversation, and the prompt box.
fn draw_sample(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    shown: &Theme,
    backdrop: Option<f32>,
    chat: bool,
    t: f32,
) {
    if area.height < 5 {
        return;
    }
    let background = shown.screen_bg.unwrap_or(shown.panel_alt);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Preview ")
        .border_style(Style::default().fg(shown.accent))
        .style(Style::default().bg(background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(dim) = backdrop {
        draw_backdrop(frame, inner, t, shown, dim);
    }
    let color = |stop: [f32; 3]| Color::Rgb(stop[0] as u8, stop[1] as u8, stop[2] as u8);
    let lines = if chat {
        vec![
            Line::from(Span::styled(
                "› add a login page",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("• ", Style::default().fg(shown.accent_soft)),
                Span::styled(
                    "Sure, starting with the form.",
                    Style::default().fg(Color::Gray),
                ),
            ]),
            Line::from(Span::styled(
                "  Tool · create_file · src/login.rs",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "Cool ",
                    Style::default()
                        .fg(color(shown.logo_primary[1]))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "Code",
                    Style::default()
                        .fg(color(shown.logo_secondary[1]))
                        .add_modifier(Modifier::BOLD),
                ),
            ])
            .centered(),
            Line::from(Span::styled(
                "a coding harness",
                Style::default().fg(shown.tagline),
            ))
            .centered(),
        ]
    };
    let text_height = (lines.len() as u16).min(inner.height);
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            text_height,
        ),
    );
    if inner.height >= text_height + 3 {
        let prompt = Rect::new(
            inner.x + 1,
            inner.y + inner.height - 3,
            inner.width.saturating_sub(2),
            3,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Type a task…",
                Style::default().fg(Color::DarkGray),
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(shown.accent))
                    .style(Style::default().bg(shown.input)),
            ),
            prompt,
        );
    }
}

fn draw_effort_preview(frame: &mut ratatui::Frame<'_>, area: Rect, animated: bool, tick: usize) {
    let mut status = vec![Span::styled(
        "effort ",
        Style::default().fg(Color::DarkGray),
    )];
    status.extend(effort_preview(animated, tick));
    let lines = vec![
        Line::from(Span::styled(
            "In the status line:",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(status),
        Line::from(""),
        Line::from(Span::styled(
            if animated {
                "The effort name keeps shining, all the time."
            } else {
                "The effort name shines for a moment after you change it, then settles."
            },
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Preview ")
                .border_style(Style::default().fg(theme::accent())),
        ),
        Rect {
            height: area.height.min(9),
            ..area
        },
    );
}

impl App {
    pub(super) fn handle_appearance_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Up => view.row = view.row.saturating_sub(1),
            KeyCode::Down => view.row = (view.row + 1).min(ROWS - 1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let settings = &mut self.settings;
                match view.row {
                    row if row < THEMES.len() => settings.theme = THEMES[row].id,
                    LIGHT_MODE => settings.light_mode = !settings.light_mode,
                    WELCOME_BACKDROP => {
                        settings.background_animation = !settings.background_animation;
                    }
                    CHAT_BACKDROP => settings.backdrop_in_chat = !settings.backdrop_in_chat,
                    DIM_IN_CHAT => {
                        settings.dim_backdrop_in_chat = !settings.dim_backdrop_in_chat;
                    }
                    _ => settings.effort_always_animated = !settings.effort_always_animated,
                }
                write_settings(&self.settings)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ANIMATE_EFFORT, CHAT_BACKDROP, LIGHT_MODE, ROWS, WELCOME_BACKDROP, effort_preview,
    };
    use crate::tui::render::draw;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{Settings, ThemeId};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    fn app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.open_settings(Section::Appearance);
        press(&mut app, KeyCode::Right);
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn down(app: &mut App, times: usize) {
        for _ in 0..times {
            press(app, KeyCode::Down);
        }
    }

    fn frame(app: &App) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(120, 34)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn enter_on_a_theme_row_applies_and_saves_that_theme() {
        let mut app = app();
        assert_eq!(app.settings.theme, ThemeId::Cool);
        down(&mut app, 1);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.theme, ThemeId::Galaxy);
        down(&mut app, 1);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.settings.theme, ThemeId::GalaxyVoid);
        assert!(crate::tui::theme::THEMES.len() >= 8);
    }

    #[test]
    fn light_mode_toggles_saves_and_repaints_the_whole_screen() {
        let mut app = app();
        assert!(!app.settings.light_mode, "dark by default");
        down(&mut app, LIGHT_MODE);
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.light_mode);
        assert!(crate::read_settings().unwrap().light_mode, "saved");
        let light = frame(&app);
        let corner = light[(0, 0)].bg;
        assert!(
            matches!(corner, Color::Rgb(r, g, b) if r > 200 && g > 200 && b > 200),
            "{corner:?}"
        );
        press(&mut app, KeyCode::Enter);
        assert!(!app.settings.light_mode);
    }

    #[test]
    fn the_backdrop_switches_toggle_independently() {
        let mut app = app();
        down(&mut app, WELCOME_BACKDROP);
        let (welcome, chat, dim) = (
            app.settings.background_animation,
            app.settings.backdrop_in_chat,
            app.settings.dim_backdrop_in_chat,
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.background_animation, !welcome);
        assert_eq!(app.settings.backdrop_in_chat, chat);
        down(&mut app, 1);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.backdrop_in_chat, !chat);
        down(&mut app, 1);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.dim_backdrop_in_chat, !dim);
        assert_eq!(app.settings.background_animation, !welcome);
    }

    #[test]
    fn the_effort_name_animation_switch_is_off_by_default_and_toggles() {
        let mut app = app();
        assert!(!app.settings.effort_always_animated);
        down(&mut app, ANIMATE_EFFORT);
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.effort_always_animated);
        press(&mut app, KeyCode::Enter);
        assert!(!app.settings.effort_always_animated);
    }

    #[test]
    fn the_effort_preview_moves_only_when_animation_is_on() {
        let words = |spans: &[ratatui::text::Span<'_>]| {
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        };
        assert_eq!(words(&effort_preview(false, 0)), "xhigh max super ultimate");
        assert_eq!(words(&effort_preview(true, 0)), "xhigh max super ultimate");
        assert_eq!(
            effort_preview(false, 0),
            effort_preview(false, 3),
            "still when off"
        );
        assert_ne!(
            effort_preview(true, 0),
            effort_preview(true, 1),
            "moving when on"
        );
    }

    #[test]
    fn the_effort_preview_sits_under_its_switch() {
        let app = app();
        let shown = text(&frame(&app));
        assert!(shown.contains("xhigh max super ultimate"), "{shown}");
    }

    #[test]
    fn a_highlighted_theme_is_previewed_in_its_own_colors_with_its_description() {
        let mut app = app();
        down(&mut app, 1);
        let buffer = frame(&app);
        let shown = text(&buffer);
        assert!(shown.contains("Preview"), "{shown}");
        assert!(shown.contains("Galaxy: Deep indigo"), "{shown}");
        let galaxy_screen = Color::Rgb(14, 10, 30);
        assert!(
            buffer.content().iter().any(|cell| cell.bg == galaxy_screen),
            "the preview is painted in Galaxy's colors while Cool is still in use"
        );
        assert_eq!(app.settings.theme, ThemeId::Cool, "browsing does not apply");
    }

    #[test]
    fn the_backdrop_rows_describe_their_current_state() {
        let mut app = app();
        down(&mut app, WELCOME_BACKDROP);
        app.settings.background_animation = false;
        assert!(text(&frame(&app)).contains("The welcome screen stays plain."));
        app.settings.background_animation = true;
        assert!(text(&frame(&app)).contains("shows the animated backdrop"));
        down(&mut app, CHAT_BACKDROP - WELCOME_BACKDROP);
        assert!(text(&frame(&app)).contains("plain background"));
    }

    #[test]
    fn a_narrow_screen_drops_the_preview_but_keeps_the_options() {
        let mut app = app();
        down(&mut app, 1);
        let mut terminal = Terminal::new(TestBackend::new(64, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        let shown = text(terminal.backend().buffer());
        assert!(!shown.contains("Preview"), "{shown}");
        assert!(
            shown.contains("Deep indigo"),
            "the summary is shown inline instead"
        );
    }

    #[test]
    fn the_cursor_stays_inside_the_section() {
        let mut app = app();
        down(&mut app, 50);
        assert_eq!(
            app.settings_view.as_ref().map(|view| view.row),
            Some(ROWS - 1)
        );
        for _ in 0..50 {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.settings_view.as_ref().map(|view| view.row), Some(0));
    }

    #[test]
    fn defaults_keep_the_original_look_and_a_calm_chat() {
        let settings = Settings::default();
        assert_eq!(settings.theme, ThemeId::Cool);
        assert!(settings.background_animation);
        assert!(!settings.backdrop_in_chat, "chats stay plain unless asked");
        assert!(settings.dim_backdrop_in_chat);
        assert!(!settings.light_mode);
    }
}
