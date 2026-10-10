mod approval;
mod backdrop;
mod chatgpt_login;
mod commands;
pub(crate) mod context;
mod creators;
mod dialog;
mod effort;
mod forms;
mod image_setup;
mod markdown;
mod mentions;
pub(crate) mod models;
mod mouse;
mod pickers;
mod present;
mod render;
mod series;
pub(crate) mod sessions;
mod settings;
mod setup;
mod state;
mod stats_view;
mod theme;
mod tracker;
mod undo;
mod usage_view;
mod usage_warnings;
mod widgets;
mod wordmark;

use crate::policy::MODES;
use crate::tui::dialog::{Routed, route};
use crate::tui::render::{draw, trust_dialog, ultimate_dialog};
use crate::tui::state::App;
use crate::{Effort, provider, read_settings, write_settings};
use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::time::Duration;

pub(crate) fn run(resume: Option<sessions::Resume>) -> Result<()> {
    // A panic would otherwise leave the terminal in raw mode on the alternate screen, which looks
    // like a frozen window; put it back first so the message can be read.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        previous(info);
    }));
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal, resume);
    restore_terminal(&mut terminal)?;
    result
}

/// Turns mouse reporting on or off, following the Mouse setting.
fn capture_mouse(on: bool) -> Result<()> {
    if on {
        execute!(io::stdout(), EnableMouseCapture).context("turning on the mouse")
    } else {
        execute!(io::stdout(), DisableMouseCapture).context("turning off the mouse")
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error).context("entering alternate screen");
    }
    Terminal::new(CrosstermBackend::new(stdout)).context("initializing TUI")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("disabling terminal raw mode")?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .context("leaving alternate screen")?;
    terminal.show_cursor().context("restoring terminal cursor")
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    resume: Option<sessions::Resume>,
) -> Result<()> {
    let mut app = App::new(read_settings()?);
    if let Some(resume) = resume {
        app.start_from(resume);
    }
    app.start_setup();
    let animation_start = std::time::Instant::now();
    let mut presenter = present::Presenter::new();
    let mut mouse_captured = false;
    while app.running {
        if app.settings.mouse != mouse_captured {
            capture_mouse(app.settings.mouse)?;
            mouse_captured = app.settings.mouse;
        }
        app.poll_response();
        let animation_tick = (animation_start.elapsed().as_millis() / 280) as usize;
        presenter.present(
            terminal,
            |frame| draw(frame, &app, animation_tick),
            || app.cursor.get(),
        )?;
        // Redraw faster while the effort picker animates or a response is streaming.
        let flashing = app
            .effort_flash_until
            .is_some_and(|until| std::time::Instant::now() < until);
        let frame_interval = Duration::from_millis(
            if app.picker
                || app.pending.is_some()
                || flashing
                || app.settings.effort_always_animated
            {
                40
            } else {
                100
            },
        );
        if !event::poll(frame_interval).context("waiting for terminal input")? {
            continue;
        }
        // Everything already waiting is read at once, so a paste can be told apart from typing.
        let mut inputs = Vec::new();
        loop {
            match event::read().context("reading terminal input")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => inputs.push(Input::Key(key)),
                Event::Mouse(mouse)
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Down(MouseButton::Left)
                            | MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                    ) =>
                {
                    inputs.push(Input::Mouse(mouse))
                }
                _ => {}
            }
            if inputs.len() >= MAX_BATCH
                || !event::poll(Duration::ZERO).context("waiting for terminal input")?
            {
                break;
            }
        }
        handle_inputs(&mut app, &inputs)?;
    }
    Ok(())
}

enum Input {
    Key(event::KeyEvent),
    Mouse(event::MouseEvent),
}

/// Handles keys and mouse events in the order they arrived; runs of keys go through
/// [`handle_batch`] together.
fn handle_inputs(app: &mut App, inputs: &[Input]) -> Result<()> {
    let mut presses = Vec::new();
    for input in inputs {
        match input {
            Input::Key(key) => presses.push(*key),
            Input::Mouse(mouse) => {
                handle_batch(app, &presses)?;
                presses.clear();
                mouse::handle_mouse(app, *mouse)?;
            }
        }
    }
    handle_batch(app, &presses)
}

impl state::App {
    /// Whether keys go to the prompt rather than to a dialog, picker or the settings screen.
    /// Mirrors the order of checks in [`handle_key`].
    fn typing_in_prompt(&self) -> bool {
        !(self.trust_prompt
            || self.chatgpt_login.is_some()
            || self.outside_prompt.is_some()
            || self.image_setup.is_some()
            || self.wizard.is_some()
            || self.tool_approval.is_some()
            || self.tracker.open
            || self.confirm_ultimate
            || self.privacy_confirmation.is_some()
            || self.chain_form.is_some()
            || self.provider_form.is_some()
            || self.model_choices.is_some()
            || self.session_picker.is_some()
            || self.model_picker.is_some()
            || self.mode_picker
            || self.picker
            || self.usage_view.is_some()
            || self.stats_view.is_some()
            || self.settings_view.is_some())
    }
}

/// Most key presses handled between two frames.
const MAX_BATCH: usize = 50_000;

/// Handles key presses that arrived together. Terminals without paste support deliver a paste as
/// typed keys; an Enter with more text right behind it is a line break inside that paste, not a
/// request to send. Nobody types the next character within the same instant.
fn handle_batch(app: &mut state::App, presses: &[event::KeyEvent]) -> Result<()> {
    for (index, key) in presses.iter().enumerate() {
        let pasted_line_break = key.code == KeyCode::Enter
            && presses[index + 1..]
                .iter()
                .any(|next| matches!(next.code, KeyCode::Char(_)));
        if pasted_line_break && app.typing_in_prompt() {
            app.input.push('\n');
            continue;
        }
        handle_key(app, *key)?;
    }
    Ok(())
}

/// Routes one key press to whichever overlay or screen currently has focus.
fn handle_key(app: &mut App, key: event::KeyEvent) -> Result<()> {
    if app.trust_prompt {
        match route(&trust_dialog(), &mut app.dialog_focus, key) {
            Routed::Press(KeyCode::Char('y')) => app.set_workspace_trusted(true)?,
            Routed::Press(_) => app.set_workspace_trusted(false)?,
            Routed::Moved | Routed::Other => {}
        }
    } else if app.chatgpt_login.is_some() {
        app.handle_chatgpt_login_key(key);
    } else if app.outside_prompt.is_some() {
        app.handle_outside_prompt_key(key)?;
    } else if app.image_setup.is_some() {
        app.handle_image_setup_key(key)?;
    } else if app.wizard.is_some() {
        app.handle_setup_key(key)?;
    } else if app.tool_approval.is_some() {
        app.handle_approval_key(key);
    } else if app.tracker.open {
        app.handle_tracker_key(key);
    } else if app.confirm_ultimate {
        match route(&ultimate_dialog(), &mut app.dialog_focus, key) {
            Routed::Press(KeyCode::Char('y')) => {
                app.settings.ultimate_acknowledged = true;
                app.apply_effort(Effort::Ultimate)?;
            }
            Routed::Press(_) => {
                app.confirm_ultimate = false;
                app.notice = "Ultimate was not selected.".to_owned();
            }
            Routed::Moved | Routed::Other => {}
        }
    } else if app.privacy_confirmation.is_some() {
        app.handle_privacy_confirmation_key(key)?;
    } else if app.chain_form.is_some() {
        app.handle_chain_form(key)?;
    } else if app.provider_form.is_some() {
        app.handle_provider_form(key)?;
    } else if app.model_choices.is_some() {
        handle_model_choice_key(app, key)?;
    } else if app.session_picker.is_some() {
        app.handle_session_picker_key(key)?;
    } else if app.model_picker.is_some() {
        app.handle_model_picker_key(key)?;
    } else if app.mode_picker {
        match key.code {
            KeyCode::Left => app.mode_index = app.mode_index.saturating_sub(1),
            KeyCode::Right => app.mode_index = (app.mode_index + 1).min(MODES.len() - 1),
            KeyCode::Enter => app.apply_mode(MODES[app.mode_index].1)?,
            KeyCode::Esc => app.mode_picker = false,
            _ => {}
        }
    } else if app.picker {
        app.handle_effort_picker_key(key)?;
    } else if app.usage_view.is_some() {
        app.handle_usage_key(key);
    } else if app.stats_view.is_some() {
        app.handle_stats_key(key)?;
    } else if app.settings_view.is_some() {
        app.handle_settings_view_key(key)?;
    } else {
        handle_prompt_key(app, key)?;
    }
    Ok(())
}

impl App {
    fn handle_privacy_confirmation_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(prompt) = self.privacy_confirmation.as_ref() else {
            return Ok(());
        };
        let has_image = self
            .pending_privacy_message
            .as_ref()
            .is_some_and(provider::message_contains_image);
        let dialog = render::privacy_dialog(prompt, has_image);
        match route(&dialog, &mut self.dialog_focus, key) {
            Routed::Press(KeyCode::Char('y')) => self.acknowledge_privacy()?,
            Routed::Press(KeyCode::Char('i')) => self.toggle_privacy_images(),
            Routed::Press(_) => {
                self.privacy_confirmation = None;
                self.pending_privacy_message = None;
                self.notice = "Request cancelled; nothing was sent.".to_owned();
            }
            Routed::Other if key.code == KeyCode::Char(' ') => self.toggle_privacy_images(),
            Routed::Moved | Routed::Other => {}
        }
        Ok(())
    }

    fn toggle_privacy_images(&mut self) {
        if let Some(prompt) = self.privacy_confirmation.as_mut() {
            prompt.allow_images = !prompt.allow_images;
        }
    }

    fn acknowledge_privacy(&mut self) -> Result<()> {
        let app = self;
        {
            {
                let prompt = app
                    .privacy_confirmation
                    .as_ref()
                    .expect("privacy confirmation open");
                let has_image = app
                    .pending_privacy_message
                    .as_ref()
                    .is_some_and(provider::message_contains_image);
                if has_image && !prompt.allow_images {
                    app.notice =
                        "Image contents remain blocked; check the consent box to include them."
                            .to_owned();
                    return Ok(());
                }
                let prompt = app
                    .privacy_confirmation
                    .take()
                    .expect("privacy confirmation open");
                let risk = prompt.risk;
                let mut changed = false;
                if !app
                    .settings
                    .privacy_acknowledged
                    .iter()
                    .any(|ack| ack == &risk)
                {
                    app.settings.privacy_acknowledged.push(risk.clone());
                    changed = true;
                }
                if prompt.allow_images
                    && !app
                        .settings
                        .privacy_image_acknowledged
                        .iter()
                        .any(|ack| ack == &risk)
                {
                    app.settings.privacy_image_acknowledged.push(risk);
                    changed = true;
                }
                if changed {
                    write_settings(&app.settings)?;
                }
                if let Some(message) = app.pending_privacy_message.take() {
                    app.dispatch_user_message(message)?;
                }
            }
        }
        Ok(())
    }
}

fn handle_model_choice_key(app: &mut App, key: event::KeyEvent) -> Result<()> {
    let Some(choices) = app.model_choices.as_ref() else {
        return Ok(());
    };
    match key.code {
        KeyCode::Up | KeyCode::Left => {
            app.model_choice_index = app.model_choice_index.saturating_sub(1)
        }
        KeyCode::Down | KeyCode::Right => {
            app.model_choice_index =
                (app.model_choice_index + 1).min(choices.len().saturating_sub(1))
        }
        KeyCode::Enter => {
            if let Some((provider_index, _, model)) = choices.get(app.model_choice_index).cloned() {
                app.activate_model(provider_index, &model)?;
            }
        }
        KeyCode::Esc => {
            app.model_choices = None;
            app.pending_model = None;
        }
        _ => {}
    }
    Ok(())
}

/// Keys while nothing is open: typing, sending, the `@` suggestions and scrolling.
fn handle_prompt_key(app: &mut App, key: event::KeyEvent) -> Result<()> {
    {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => app.open_tracker(),
            KeyCode::Up if app.mention.is_some() && !control => app.mention_move(-1),
            KeyCode::Down if app.mention.is_some() && !control => app.mention_move(1),
            KeyCode::Tab if app.mention.is_some() => app.accept_mention(),
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                app.input.push('\n')
            }
            KeyCode::Enter if app.mention_takes_enter() => app.accept_mention(),
            KeyCode::Esc if app.mention.is_some() => app.dismiss_mention(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.running = false;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::ALT) => {
                app.toggle_chain()?
            }
            KeyCode::Esc if app.streaming.is_some() => app.cancel_turn(),
            KeyCode::Esc => app.running = false,
            KeyCode::Enter => app.submit()?,
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.history_scroll = app.history_scroll.saturating_add(3)
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.history_scroll = app.history_scroll.saturating_sub(3)
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.input.push(character);
            }
            _ => {}
        }
        app.refresh_mentions();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::handle_key;
    use crate::Settings;
    use crate::agent::{PendingEvent, ToolApproval};
    use crate::tui::state::{App, StreamingTurn, TranscriptKind};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn streaming_app(text: &str) -> (App, mpsc::Sender<PendingEvent>, Arc<AtomicBool>) {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut turn = StreamingTurn::new(cancel.clone());
        turn.text = text.to_owned();
        app.streaming = Some(turn);
        (app, sender, cancel)
    }

    fn key_press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(text: &str) -> Vec<KeyEvent> {
        text.chars()
            .map(|character| match character {
                '\n' => key_press(KeyCode::Enter),
                other => key_press(KeyCode::Char(other)),
            })
            .collect()
    }

    #[test]
    fn a_pasted_block_of_lines_stays_in_the_prompt_instead_of_being_sent_line_by_line() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        super::handle_batch(&mut app, &typed("fn main() {\n    run();\n}")).unwrap();
        assert_eq!(app.input, "fn main() {\n    run();\n}");
        assert!(app.transcript.is_empty(), "nothing was sent");
        assert!(app.pending.is_none());
    }

    #[test]
    fn an_enter_on_its_own_still_sends() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        super::handle_batch(&mut app, &typed("/help")).unwrap();
        super::handle_batch(&mut app, &typed("\n")).unwrap();
        assert!(app.input.is_empty(), "the command ran: {:?}", app.input);
        // A key that is not text behind the Enter (say an arrow) does not make it a paste.
        app.input = "/help".to_owned();
        super::handle_batch(
            &mut app,
            &[key_press(KeyCode::Enter), key_press(KeyCode::Up)],
        )
        .unwrap();
        assert!(app.input.is_empty());
    }

    #[test]
    fn shift_or_alt_enter_starts_a_new_line() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.input = "first".to_owned();
        for modifier in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
            handle_key(&mut app, KeyEvent::new(KeyCode::Enter, modifier)).unwrap();
        }
        assert_eq!(app.input, "first\n\n");
    }

    #[test]
    fn line_breaks_in_the_prompt_start_new_rows_and_move_the_cursor() {
        let (lines, cursor) = crate::tui::render::wrap_input_text("ab\ncd", 40);
        assert_eq!(lines, ["ab", "cd"]);
        assert_eq!(cursor, (1, 2));
        let (lines, cursor) = crate::tui::render::wrap_input_text("ab\n", 40);
        assert_eq!(lines, ["ab", ""]);
        assert_eq!(cursor, (1, 0));
    }

    #[test]
    fn esc_cancels_running_turn_and_keeps_partial_text() {
        let (mut app, _sender, cancel) = streaming_app("partial answer");
        handle_key(&mut app, esc()).expect("esc");
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.running);
        assert!(app.streaming.is_none() && app.pending.is_none());
        let kinds = app
            .transcript
            .iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![TranscriptKind::Assistant, TranscriptKind::CommandOutput]
        );
        assert_eq!(app.transcript[0].text, "partial answer");
        assert_eq!(app.transcript[1].text, "(interrupted)");
        let context = app.messages.last().expect("context message");
        assert_eq!(context.role, "assistant");
        assert!(
            context
                .content
                .as_str()
                .is_some_and(|text| text.ends_with("[interrupted by the user]"))
        );
    }

    #[test]
    fn late_events_after_cancel_are_ignored() {
        let (mut app, sender, _) = streaming_app("partial");
        handle_key(&mut app, esc()).expect("esc");
        let before = app.transcript.len();
        let _ = sender.send(PendingEvent::TextDelta("late".to_owned()));
        let _ = sender.send(PendingEvent::Finished(Err("cancelled".to_owned())));
        app.poll_response();
        assert_eq!(app.transcript.len(), before);
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    }

    fn setup_app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.start_setup();
        app
    }

    #[test]
    fn the_setup_wizard_takes_keys_before_the_prompt() {
        let mut app = setup_app();
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.input, "", "typing does not reach the prompt");
        press(&mut app, KeyCode::Enter);
        assert!(
            app.settings.theme_prompt_answered,
            "Enter answered the first step"
        );
    }

    #[test]
    fn escape_in_the_wizard_skips_setup_instead_of_quitting() {
        let mut app = setup_app();
        press(&mut app, KeyCode::Esc);
        assert!(app.wizard.is_none());
        assert!(app.running, "Esc answers the question instead of quitting");
    }

    #[test]
    fn the_setup_wizard_waits_for_the_workspace_trust_question() {
        let mut app = setup_app();
        app.trust_prompt = true;
        press(&mut app, KeyCode::Char('y'));
        // The key went to the trust prompt, so the wizard has not moved.
        assert!(app.wizard.is_some() && !app.settings.theme_prompt_answered);
        assert!(!app.trust_prompt);
    }

    #[test]
    fn esc_without_running_turn_still_quits() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        handle_key(&mut app, esc()).expect("esc");
        assert!(!app.running);
    }

    #[test]
    fn esc_during_approval_answers_the_prompt() {
        let (mut app, _sender, cancel) = streaming_app("working");
        let (response, decision) = mpsc::sync_channel(1);
        app.tool_approval = Some(ToolApproval {
            title: "Run shell command".to_owned(),
            details: "cargo test".to_owned(),
            response,
        });
        handle_key(&mut app, esc()).expect("esc");
        assert_eq!(decision.try_recv(), Ok(false));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(app.streaming.is_some());
    }

    #[test]
    fn mid_stream_error_keeps_partial_text() {
        let (mut app, sender, _) = streaming_app("");
        sender
            .send(PendingEvent::TextDelta("half an answer".to_owned()))
            .unwrap();
        sender
            .send(PendingEvent::Finished(Err(
                "provider error: overloaded".to_owned()
            )))
            .unwrap();
        app.poll_response();
        let kinds = app
            .transcript
            .iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                TranscriptKind::Assistant,
                TranscriptKind::CommandOutput,
                TranscriptKind::Error
            ]
        );
        assert_eq!(app.transcript[0].text, "half an answer");
    }
}
