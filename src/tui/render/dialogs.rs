use crate::policy::MODES;
use crate::tui::dialog::{Button, Dialog, Tone, draw_dialog, hint_style, window};
use crate::tui::mouse::{Click, Row as MouseRow, record_wrapped};
use crate::tui::render::{centered_rect, mode_span};
use crate::tui::state::{App, PrivacyPrompt};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph, Wrap};

/// The one-time privacy acknowledgement before a request to a flagged provider. `i` (or Space)
/// toggles the separate consent for images.
pub(in crate::tui) fn privacy_dialog(prompt: &PrivacyPrompt, has_image: bool) -> Dialog<'static> {
    let lines = vec![
        Line::from(Span::styled(
            format!(
                "{} requests send prompt context to that provider.",
                prompt.risk
            ),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(
            "Before sending, Cool Code locally redacts detected API keys, email addresses, phone-like numbers, and your custom values. The reversible mapping stays in memory and is never sent; matching placeholders in the reply are restored locally.",
        ),
        Line::from(""),
        Line::from(
            "This is best-effort, not a guarantee. Image contents are not scanned or redacted and may expose sensitive information.",
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                if prompt.allow_images {
                    "[✓] "
                } else {
                    "[ ] "
                },
                Style::default().fg(if prompt.allow_images {
                    Color::Green
                } else {
                    Color::Gray
                }),
            ),
            Span::styled(
                "Allow image contents to be sent unredacted",
                Style::default().fg(Color::White),
            ),
        ]),
    ];
    let mut lines = lines;
    if has_image && !prompt.allow_images {
        lines.push(Line::from(Span::styled(
            "An image is attached; enable this option before continuing.",
            Style::default().fg(Color::Rgb(255, 197, 92)),
        )));
    }
    Dialog {
        id: "privacy",
        title: "Privacy check".to_owned(),
        tone: Tone::Warning,
        body: lines,
        buttons: vec![
            Button::new("Allow images", 'i'),
            Button::new("Acknowledge and send", 'y'),
            Button::new("Cancel", 'n'),
        ],
        cancel: 2,
        default: 2,
    }
}

pub(super) fn draw_privacy_confirmation(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    prompt: &PrivacyPrompt,
    has_image: bool,
) {
    let dialog = privacy_dialog(prompt, has_image);
    draw_dialog(frame, area, &dialog, &app.dialog_focus, &app.hits);
}

/// The question whether to trust the working folder.
pub(in crate::tui) fn trust_dialog() -> Dialog<'static> {
    let root = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "(unknown folder)".to_owned());
    let cool_file = std::env::current_dir()
        .ok()
        .is_some_and(|path| path.join("COOL.md").is_file());
    let cool_dir = std::env::current_dir()
        .ok()
        .is_some_and(|path| path.join(".coolcode").is_dir());
    let lines = vec![
        Line::from(Span::styled(
            "Cool Code has not recorded trust for this folder yet.",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(root),
        Line::from(format!(
            "COOL.md: {}   ·   .coolcode/: {}",
            if cool_file { "present" } else { "not present" },
            if cool_dir { "present" } else { "not present" }
        )),
        Line::from(""),
        Line::from(
            "Trusting enables repository reads and permission-controlled exact-snippet edits and shell commands in this folder. The active permission mode determines which actions auto-run and which require your explicit approval. COOL.md and explicit @path files can be read only in a trusted workspace.",
        ),
        Line::from(
            "Declining keeps chat available but disables COOL.md and @path file reads. You can change this later in Settings → Privacy.",
        ),
    ];
    Dialog::confirm(
        "trust",
        "Trust this workspace?",
        Tone::Warning,
        lines,
        "Yes, trust this folder",
        "No",
    )
}

pub(super) fn draw_workspace_trust_prompt(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    draw_dialog(frame, area, &trust_dialog(), &app.dialog_focus, &app.hits);
}

pub(super) fn draw_model_provider_picker(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let choices = app
        .model_choices
        .as_ref()
        .expect("model provider picker open");
    let popup = centered_rect(62, (choices.len() as u16 * 3 + 8).min(70), area);
    frame.render_widget(Clear, popup);
    let block = window("Choose provider", Tone::Normal).padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    app.hits.wheel_arrows(popup);
    let model = app.pending_model.as_deref().unwrap_or("model");
    let mut lines = vec![
        Line::from(format!("`{model}` is available from multiple providers:")),
        Line::from(""),
    ];
    let mut clicks = Vec::new();
    for (index, (provider_index, name, _model_id)) in choices.iter().enumerate() {
        clicks.push((
            lines.len(),
            Click::Row(MouseRow::new(index, app.model_choice_index)),
        ));
        let profile = &app.settings.providers[*provider_index];
        lines.push(Line::from(vec![
            Span::styled(
                if index == app.model_choice_index {
                    "› "
                } else {
                    "  "
                },
                Style::default().fg(crate::tui::theme::accent()),
            ),
            Span::styled(name, Style::default().fg(Color::White)),
            Span::styled(
                format!("  ·  {}", profile.adapter),
                Style::default().fg(Color::Gray),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ choose · Enter select · Esc cancel",
        hint_style(),
    )));
    record_wrapped(&app.hits, inner, &lines, &clicks);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// The gap between two modes in the mode picker.
const MODE_GAP: &str = "   ·   ";

pub(super) fn draw_mode_picker(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    use unicode_width::UnicodeWidthStr as _;
    let popup = centered_rect(72, 34, area);
    frame.render_widget(Clear, popup);
    let block = window("Permission mode", Tone::Normal).padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let auto_unusable = !app.settings.auto_ready();
    let options = MODES
        .iter()
        .enumerate()
        .map(|(index, (_, mode))| {
            let mut span = mode_span(mode, index == app.mode_index);
            if *mode == "auto" && auto_unusable {
                span.style = Style::default().fg(Color::DarkGray);
            }
            if index == app.mode_index {
                span.content = format!("[ {} ]", span.content).into();
            }
            span
        })
        .collect::<Vec<_>>();
    // The modes are packed into as few centered rows as fit, so each one's place is known.
    let mut rows: Vec<Vec<(usize, Span<'static>)>> = vec![Vec::new()];
    let mut used = 0usize;
    let gap = MODE_GAP.width();
    for (index, option) in options.into_iter().enumerate() {
        let width = option.content.width();
        let row = rows.last_mut().expect("a row");
        if !row.is_empty() && used + gap + width > usize::from(inner.width) {
            rows.push(vec![(index, option)]);
            used = width;
        } else {
            used += if row.is_empty() { width } else { gap + width };
            row.push((index, option));
        }
    }
    let mut lines = vec![Line::from("")];
    for (row_index, row) in rows.iter().enumerate() {
        let width = row
            .iter()
            .map(|(_, span)| span.content.width())
            .sum::<usize>()
            + gap * row.len().saturating_sub(1);
        let mut x = inner.x + inner.width.saturating_sub(width as u16) / 2;
        let y = inner.y + 1 + row_index as u16;
        let mut spans = Vec::new();
        for (position, (index, span)) in row.iter().enumerate() {
            if position > 0 {
                spans.push(Span::raw(MODE_GAP));
                x += gap as u16;
            }
            let span_width = span.content.width() as u16;
            if y < inner.bottom() {
                app.hits.click(
                    Rect::new(x, y, span_width.min(inner.right().saturating_sub(x)), 1),
                    Click::Row(MouseRow::new(*index, app.mode_index).horizontal()),
                );
            }
            x += span_width;
            spans.push(span.clone());
        }
        lines.push(Line::from(spans));
    }
    let selected = MODES[app.mode_index].1;
    lines.extend([
        Line::from(""),
        Line::from(vec![
            Span::styled("Current: ", Style::default().fg(Color::Gray)),
            mode_span(selected, true),
        ]),
        Line::from(Span::styled(
            "←/→ browse   Enter select   Esc cancel",
            hint_style(),
        )),
    ]);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

/// The one-time warning before the Ultimate effort is first chosen.
pub(in crate::tui) fn ultimate_dialog() -> Dialog<'static> {
    Dialog::confirm(
        "ultimate",
        "Confirm Ultimate effort",
        Tone::Danger,
        vec![Line::from(
            "Ultimate runs at the model's highest effort and lets the assistant start as many subagents as your Dynamic workflows size allows, and have its work reviewed twice. It can use many times more tokens and cost much more than a normal turn.",
        )],
        "Use Ultimate",
        "Cancel",
    )
}

pub(super) fn draw_ultimate_confirmation(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    draw_dialog(
        frame,
        area,
        &ultimate_dialog(),
        &app.dialog_focus,
        &app.hits,
    );
}

#[cfg(test)]
mod tests {
    use crate::tui::render::draw;
    use crate::tui::state::{App, PrivacyPrompt};
    use crate::{Settings, provider};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn the_ultimate_warning_points_at_the_workflow_size_instead_of_a_fixed_number() {
        let mut terminal = Terminal::new(TestBackend::new(100, 32)).expect("test terminal");
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.confirm_ultimate = true;
        terminal
            .draw(|frame| draw(frame, &app, 0))
            .expect("draw the warning");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Confirm Ultimate"), "{rendered}");
        assert!(rendered.contains("workflows size"), "{rendered}");
        assert!(!rendered.contains("6 subagents"), "{rendered}");
    }

    fn app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app
    }

    fn press(app: &mut App, code: crossterm::event::KeyCode) {
        crate::tui::handle_key(
            app,
            crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
        )
        .expect("key");
    }

    use crate::tui::mouse::testing::{click_text, drawn, has_button, rows};
    use crossterm::event::KeyCode;

    /// The dialog's outline: its top and bottom rows, found by the rounded corners.
    fn dialog_rows(app: &App) -> (usize, usize) {
        let shown = rows(&drawn(app, 100, 30));
        let top = shown.iter().position(|row| row.contains('╭')).expect("top");
        let bottom = shown
            .iter()
            .rposition(|row| row.contains('╰'))
            .expect("bottom");
        (top, bottom)
    }

    #[test]
    fn the_trust_question_is_a_shared_dialog_with_the_same_answers() {
        let mut asking = app();
        asking.trust_prompt = true;
        assert!(has_button(&asking, "Yes, trust this folder"));
        assert!(has_button(&asking, "No"));
        let (top, bottom) = dialog_rows(&asking);
        assert!(
            bottom - top < 20,
            "short, not a tall popup: {top}..{bottom}"
        );
        for (keys, trusted) in [
            (vec![KeyCode::Enter], false),
            (vec![KeyCode::Esc], false),
            (vec![KeyCode::Char('n')], false),
            (vec![KeyCode::Left, KeyCode::Enter], true),
        ] {
            let mut app = app();
            app.trust_prompt = true;
            app.projects_path =
                std::env::temp_dir().join(format!("trust-{}.toml", uuid::Uuid::new_v4()));
            for key in &keys {
                press(&mut app, *key);
            }
            assert!(!app.trust_prompt, "{keys:?} answers");
            assert_eq!(app.workspace_trusted, trusted, "{keys:?}");
        }
        let mut clicked = app();
        clicked.trust_prompt = true;
        clicked.projects_path =
            std::env::temp_dir().join(format!("trust-{}.toml", uuid::Uuid::new_v4()));
        click_text(&mut clicked, "Yes, trust this folder");
        assert!(clicked.workspace_trusted && !clicked.trust_prompt);
    }

    #[test]
    fn the_ultimate_warning_is_a_shared_dialog_with_the_same_answers() {
        let unlocked = || {
            let mut app = app();
            app.settings.workflow_size = crate::workflow::WorkflowSize::Medium;
            app.confirm_ultimate = true;
            app
        };
        assert!(has_button(&unlocked(), "Use Ultimate"));
        for key in [KeyCode::Char('n'), KeyCode::Esc, KeyCode::Enter] {
            let mut app = unlocked();
            press(&mut app, key);
            assert!(!app.confirm_ultimate, "{key:?}");
            assert_eq!(app.notice, "Ultimate was not selected.", "{key:?}");
            assert!(!app.settings.ultimate_acknowledged);
        }
        let mut app = unlocked();
        press(&mut app, KeyCode::Char('y'));
        assert!(app.settings.ultimate_acknowledged && !app.confirm_ultimate);
        assert_eq!(app.settings.effort, crate::Effort::Ultimate);
        let mut app = unlocked();
        click_text(&mut app, "Use Ultimate");
        assert_eq!(app.settings.effort, crate::Effort::Ultimate);
    }

    #[test]
    fn the_privacy_check_is_a_shared_dialog_with_the_same_answers() {
        let asking = || {
            let mut app = app();
            app.privacy_confirmation = Some(PrivacyPrompt {
                risk: "Google/Gemini".to_owned(),
                allow_images: false,
            });
            app
        };
        assert!(has_button(&asking(), "Acknowledge and send"));
        for key in [KeyCode::Char('n'), KeyCode::Esc, KeyCode::Enter] {
            let mut app = asking();
            app.pending_privacy_message = Some(provider::ChatMessage::user_with_images(
                "hi".to_owned(),
                "hi".to_owned(),
                Vec::new(),
            ));
            press(&mut app, key);
            assert!(app.privacy_confirmation.is_none(), "{key:?}");
            assert!(app.pending_privacy_message.is_none(), "{key:?}");
            assert_eq!(app.notice, "Request cancelled; nothing was sent.");
        }
        let mut app = asking();
        for key in [KeyCode::Char('i'), KeyCode::Char(' ')] {
            press(&mut app, key);
        }
        assert!(
            !app.privacy_confirmation.as_ref().unwrap().allow_images,
            "i and Space each toggle the image consent"
        );
        click_text(&mut app, "Allow images");
        assert!(app.privacy_confirmation.as_ref().unwrap().allow_images);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.privacy_confirmation.is_none());
        assert!(
            app.settings
                .privacy_acknowledged
                .contains(&"Google/Gemini".to_owned())
        );
        assert!(
            app.settings
                .privacy_image_acknowledged
                .contains(&"Google/Gemini".to_owned())
        );
    }

    #[test]
    fn privacy_dialog_offers_separate_image_consent() {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.privacy_confirmation = Some(PrivacyPrompt {
            risk: "Google/Gemini".to_owned(),
            allow_images: false,
        });
        app.pending_privacy_message = Some(provider::ChatMessage::user_with_images(
            "image request".to_owned(),
            "look".to_owned(),
            vec![
                serde_json::json!({"type":"image_url", "image_url":{"url":"data:image/png;base64,aGVsbG8="}}),
            ],
        ));
        terminal
            .draw(|frame| draw(frame, &app, 0))
            .expect("draw privacy prompt");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Allow image contents"));
        assert!(rendered.contains("enable this option"));
    }
}
