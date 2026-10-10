//! Setting up the image API behind the `generate_image` tool.
//!
//! Three fields (address, model, key) and a way to turn it off. The key goes to the OS
//! credential store; the address and model go to the settings file. Until this is filled in, the
//! model is never offered the tool.

use crate::imagegen::{DEFAULT_BASE_URL, DEFAULT_MODEL, ImageConfig, key_name};
use crate::tui::forms::normalize_api_key;
use crate::tui::render::centered_rect;
use crate::tui::state::{App, edit_string};
use crate::write_settings;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

/// The text fields, in order, followed by the buttons.
const FIELD_COUNT: usize = 3;
const SAVE: usize = 3;
const TURN_OFF: usize = 4;
const CANCEL: usize = 5;
const FOCUS_COUNT: usize = 6;

pub(in crate::tui) struct ImageSetup {
    base_url: String,
    model: String,
    key: String,
    focus: usize,
    /// A key is already saved, so a blank key field keeps it.
    had_key: bool,
    error: Option<String>,
}

impl App {
    pub(in crate::tui) fn open_image_setup(&mut self) {
        let (base_url, model) = match &self.settings.image_generation {
            Some(config) => (config.base_url.clone(), config.model.clone()),
            None => (DEFAULT_BASE_URL.to_owned(), DEFAULT_MODEL.to_owned()),
        };
        self.image_setup = Some(ImageSetup {
            base_url,
            model,
            key: String::new(),
            focus: 0,
            had_key: crate::imagegen::saved_key().is_some(),
            error: None,
        });
    }

    pub(in crate::tui) fn handle_image_setup_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(setup) = self.image_setup.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Esc => self.image_setup = None,
            KeyCode::Tab | KeyCode::Down => setup.focus = (setup.focus + 1) % FOCUS_COUNT,
            KeyCode::Up | KeyCode::BackTab => {
                setup.focus = (setup.focus + FOCUS_COUNT - 1) % FOCUS_COUNT;
            }
            KeyCode::Enter => match setup.focus {
                0 | 1 => setup.focus += 1,
                2 => setup.focus = SAVE,
                SAVE => self.save_image_setup()?,
                TURN_OFF => self.turn_off_images()?,
                _ => self.image_setup = None,
            },
            _ if setup.focus < FIELD_COUNT => {
                setup.error = None;
                match setup.focus {
                    0 => edit_string(&mut setup.base_url, key),
                    1 => edit_string(&mut setup.model, key),
                    _ => edit_string(&mut setup.key, key),
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn save_image_setup(&mut self) -> Result<()> {
        let Some(setup) = self.image_setup.as_mut() else {
            return Ok(());
        };
        let base_url = setup.base_url.trim().trim_end_matches('/').to_owned();
        let model = setup.model.trim().to_owned();
        let typed_key = normalize_api_key(&setup.key);
        let fail = |setup: &mut ImageSetup, message: &str| {
            setup.error = Some(message.to_owned());
        };
        if base_url.is_empty() || model.is_empty() {
            fail(setup, "Both the address and the model are needed.");
            return Ok(());
        }
        if let Err(error) = crate::endpoints::resolve_endpoint(&base_url, "images/generations") {
            let message = format!("That address cannot be used: {error}");
            fail(setup, &message);
            return Ok(());
        }
        if typed_key.is_empty() && !setup.had_key {
            fail(setup, "Add the API key for this service.");
            return Ok(());
        }
        if !typed_key.is_empty() {
            crate::secrets::store(&key_name(), &typed_key)?;
        }
        self.settings.image_generation = Some(ImageConfig { base_url, model });
        write_settings(&self.settings)?;
        self.image_setup = None;
        self.notice = "Image generation is on. The model can now make placeholder images; each one asks you first unless the mode is Accept Everything.".to_owned();
        Ok(())
    }

    fn turn_off_images(&mut self) -> Result<()> {
        self.settings.image_generation = None;
        write_settings(&self.settings)?;
        crate::secrets::delete(&key_name())?;
        self.image_setup = None;
        self.notice = "Image generation is off and its key was deleted.".to_owned();
        Ok(())
    }
}

pub(in crate::tui) fn draw_image_setup(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    setup: &ImageSetup,
) {
    let popup = centered_rect(74, 70, area);
    frame.render_widget(Clear, popup);
    let block = crate::tui::dialog::window("Image generation", crate::tui::dialog::Tone::Normal)
        .padding(ratatui::widgets::Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from(Span::styled(
            "Lets the model make placeholder pictures while it builds (a hero image, an icon). It uses an OpenAI-style images API that you provide; every image costs money there, and each one asks you first unless the mode is Accept Everything.",
            dim,
        )),
        Line::from(""),
    ];
    let field = |index: usize, label: &str, value: String| {
        let current = setup.focus == index;
        vec![
            Line::from(vec![
                Span::styled(
                    if current { "› " } else { "  " },
                    Style::default().fg(crate::tui::theme::accent()),
                ),
                Span::styled(
                    label.to_owned(),
                    if current {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
            ]),
            Line::from(Span::styled(
                format!("    {value}"),
                Style::default().fg(Color::Rgb(185, 195, 205)),
            )),
            Line::from(""),
        ]
    };
    lines.extend(field(0, "API address", setup.base_url.clone()));
    lines.extend(field(1, "Model", setup.model.clone()));
    let masked = if setup.key.is_empty() {
        if setup.had_key {
            "(leave blank to keep the saved key)".to_owned()
        } else {
            String::new()
        }
    } else {
        "•".repeat(setup.key.chars().count().min(42))
    };
    lines.extend(field(
        2,
        "API key · kept in the OS credential store",
        masked,
    ));
    let button = |index: usize, label: &str| {
        let current = setup.focus == index;
        Span::styled(
            format!("[ {label} ]  "),
            if current {
                Style::default()
                    .fg(Color::White)
                    .bg(crate::tui::theme::accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        )
    };
    lines.push(Line::from(vec![
        button(SAVE, "Save"),
        button(TURN_OFF, "Turn off"),
        button(CANCEL, "Cancel"),
    ]));
    if let Some(error) = &setup.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Rgb(235, 80, 80)),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tab or ↑/↓ move · Enter continues · Esc closes",
        dim,
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            app.handle_image_setup_key(key(KeyCode::Char(character)))
                .expect("type");
        }
    }

    fn screen(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 40)).expect("terminal");
        terminal
            .draw(|frame| crate::tui::render::draw(frame, app, 0))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn fresh_app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        let _ = crate::secrets::delete(&key_name());
        app
    }

    /// Fills the key field and saves, with the address and model as they are.
    fn save_with_key(app: &mut App, key_text: &str) {
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        type_text(app, key_text);
        app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
    }

    #[test]
    fn the_form_starts_with_the_usual_address_and_model_and_nothing_else_is_on() {
        let mut app = fresh_app();
        assert!(!crate::imagegen::available(&app.settings));
        app.open_image_setup();
        let shown = screen(&app);
        assert!(shown.contains("Image generation"), "{shown}");
        assert!(
            shown.contains(DEFAULT_BASE_URL) && shown.contains(DEFAULT_MODEL),
            "{shown}"
        );
        assert!(
            shown.contains("Accept Everything"),
            "the cost warning is shown: {shown}"
        );
    }

    #[test]
    fn saving_stores_the_key_safely_and_turns_the_tool_on() {
        let mut app = fresh_app();
        app.open_image_setup();
        save_with_key(&mut app, "sk-image-secret-123");
        assert!(app.image_setup.is_none(), "the form closed");
        assert_eq!(
            app.settings.image_generation,
            Some(ImageConfig {
                base_url: DEFAULT_BASE_URL.to_owned(),
                model: DEFAULT_MODEL.to_owned(),
            })
        );
        assert_eq!(
            crate::secrets::load(&key_name()).unwrap().as_deref(),
            Some("sk-image-secret-123")
        );
        assert!(crate::imagegen::available(&app.settings));
        assert!(
            !app.notice.contains("sk-image-secret-123"),
            "{}",
            app.notice
        );
        let on_disk = toml::to_string(&app.settings).unwrap();
        assert!(
            !on_disk.contains("sk-image-secret-123"),
            "the key never reaches the settings file"
        );
        let _ = crate::secrets::delete(&key_name());
    }

    #[test]
    fn the_key_is_hidden_while_typing() {
        let mut app = fresh_app();
        app.open_image_setup();
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        type_text(&mut app, "sk-visible-if-broken");
        let shown = screen(&app);
        assert!(!shown.contains("sk-visible-if-broken"), "{shown}");
        assert!(shown.contains("••••"), "{shown}");
    }

    #[test]
    fn saving_without_any_key_is_refused_but_a_saved_key_is_kept_when_left_blank() {
        let mut app = fresh_app();
        app.open_image_setup();
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
        assert!(app.image_setup.is_some(), "still open");
        assert!(screen(&app).contains("Add the API key"));
        assert!(app.settings.image_generation.is_none());
        // With a key already saved, a blank field keeps it and can still change the model.
        crate::secrets::store(&key_name(), "kept-key").unwrap();
        app.open_image_setup();
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        for _ in 0..DEFAULT_MODEL.len() {
            app.handle_image_setup_key(key(KeyCode::Backspace)).unwrap();
        }
        type_text(&mut app, "another-model");
        // Past the model and the (blank) key to Save, then save.
        for _ in 0..3 {
            app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
        }
        assert!(app.image_setup.is_none());
        assert_eq!(
            app.settings.image_generation.as_ref().unwrap().model,
            "another-model"
        );
        assert_eq!(
            crate::secrets::load(&key_name()).unwrap().as_deref(),
            Some("kept-key")
        );
        let _ = crate::secrets::delete(&key_name());
    }

    #[test]
    fn an_unusable_address_is_explained_and_nothing_is_saved() {
        let mut app = fresh_app();
        app.open_image_setup();
        for _ in 0..DEFAULT_BASE_URL.len() {
            app.handle_image_setup_key(key(KeyCode::Backspace)).unwrap();
        }
        type_text(&mut app, "http://images.example/v1");
        save_with_key_from_address(&mut app);
        assert!(app.image_setup.is_some());
        assert!(screen(&app).contains("cannot be used"), "{}", screen(&app));
        assert!(app.settings.image_generation.is_none());
        assert!(
            crate::secrets::load(&key_name()).unwrap().is_none(),
            "no key saved for a refused setup"
        );
    }

    /// From the address field: go to the key, type one, and try to save.
    fn save_with_key_from_address(app: &mut App) {
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        type_text(app, "some-key");
        app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
        app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
    }

    #[test]
    fn turning_it_off_removes_the_setup_and_the_key_and_cancel_changes_nothing() {
        let mut app = fresh_app();
        app.open_image_setup();
        save_with_key(&mut app, "to-be-deleted");
        assert!(crate::imagegen::available(&app.settings));
        app.open_image_setup();
        app.handle_image_setup_key(key(KeyCode::Esc)).unwrap();
        assert!(
            app.image_setup.is_none() && crate::imagegen::available(&app.settings),
            "cancel keeps it"
        );
        app.open_image_setup();
        for _ in 0..TURN_OFF {
            app.handle_image_setup_key(key(KeyCode::Tab)).unwrap();
        }
        app.handle_image_setup_key(key(KeyCode::Enter)).unwrap();
        assert!(app.settings.image_generation.is_none());
        assert!(crate::secrets::load(&key_name()).unwrap().is_none());
        assert!(!crate::imagegen::available(&app.settings));
    }

    #[test]
    fn settings_general_opens_the_form_and_shows_whether_it_is_on() {
        use crate::tui::settings::Section;
        let mut app = fresh_app();
        app.open_settings(Section::General);
        app.handle_settings_view_key(key(KeyCode::Right)).unwrap();
        // The row just above Reset.
        let row = 14;
        for _ in 0..row {
            app.handle_settings_view_key(key(KeyCode::Down)).unwrap();
        }
        let shown = screen(&app);
        assert!(shown.contains("Image generation"), "{shown}");
        app.handle_settings_view_key(key(KeyCode::Enter)).unwrap();
        assert!(app.image_setup.is_some(), "Enter opens the form");
    }
}
