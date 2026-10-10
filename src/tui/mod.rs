mod backdrop;
mod chatgpt_login;
mod commands;
pub(crate) mod context;
mod creators;
mod editing;
mod effort;
mod extensions;
mod forms;
mod image_setup;
mod markdown;
mod mentions;
pub(crate) mod models;
mod pickers;
mod plugin_install;
mod present;
mod render;
mod series;
pub(crate) mod sessions;
mod settings;
mod setup;
mod slash;
mod state;
mod stats_view;
mod theme;
mod undo;
mod usage_view;
mod usage_warnings;
mod widgets;
mod wordmark;

use crate::policy::MODES;
use crate::tui::render::draw;
use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
use crate::{Effort, provider, read_settings, write_settings};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
        previous(info);
    }));
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal, resume);
    restore_terminal(&mut terminal)?;
    result
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
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leaving alternate screen")?;
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
    app.start_mods();
    let animation_start = std::time::Instant::now();
    let mut presenter = present::Presenter::new();
    while app.running {
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
        let mut presses = Vec::new();
        loop {
            if let Event::Key(key) = event::read().context("reading terminal input")?
                && key.kind == KeyEventKind::Press
            {
                presses.push(key);
            }
            if presses.len() >= MAX_BATCH
                || !event::poll(Duration::ZERO).context("waiting for terminal input")?
            {
                break;
            }
        }
        handle_batch(&mut app, &presses)?;
    }
    app.stop_mods();
    Ok(())
}

impl state::App {
    /// Whether keys go to the prompt rather than to a dialog, picker or the settings screen.
    /// Mirrors the order of checks in [`handle_key`].
    fn typing_in_prompt(&self) -> bool {
        !(self.trust_prompt
            || self.chatgpt_login.is_some()
            || self.outside_prompt.is_some()
            || self.plugin_review_open()
            || self.image_setup.is_some()
            || self.wizard.is_some()
            || self.tool_approval.is_some()
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
            app.insert_input("\n");
            continue;
        }
        handle_key(app, *key)?;
    }
    Ok(())
}

/// Routes one key press to whichever overlay or screen currently has focus.
fn handle_key(app: &mut App, key: event::KeyEvent) -> Result<()> {
    if app.trust_prompt {
        match key.code {
            KeyCode::Left => app.trust_choice = 0,
            KeyCode::Right => app.trust_choice = 1,
            KeyCode::Char('y' | 'Y') => app.set_workspace_trusted(true)?,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => app.set_workspace_trusted(false)?,
            KeyCode::Enter if app.trust_choice == 0 => app.set_workspace_trusted(true)?,
            KeyCode::Enter => app.set_workspace_trusted(false)?,
            _ => {}
        }
    } else if app.chatgpt_login.is_some() {
        app.handle_chatgpt_login_key(key);
    } else if app.outside_prompt.is_some() {
        app.handle_outside_prompt_key(key)?;
    } else if app.plugin_review_open() {
        app.handle_plugin_review_key(key)?;
    } else if app.image_setup.is_some() {
        app.handle_image_setup_key(key)?;
    } else if app.wizard.is_some() {
        app.handle_setup_key(key)?;
    } else if app.tool_approval.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                if let Some(approval) = app.tool_approval.take() {
                    let _ = approval.response.send(true);
                    app.transcript.push(TranscriptEntry {
                        kind: TranscriptKind::CommandOutput,
                        text: format!("Approved: {}", approval.title),
                    });
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                if let Some(approval) = app.tool_approval.take() {
                    let _ = approval.response.send(false);
                    app.transcript.push(TranscriptEntry {
                        kind: TranscriptKind::CommandOutput,
                        text: format!("Declined: {}", approval.title),
                    });
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.approval_scroll = app.approval_scroll.saturating_add(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.approval_scroll = app.approval_scroll.saturating_sub(1)
            }
            _ => {}
        }
    } else if app.confirm_ultimate {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                app.settings.ultimate_acknowledged = true;
                app.apply_effort(Effort::Ultimate)?;
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                app.confirm_ultimate = false;
                app.notice = "Ultimate was not selected.".to_owned();
            }
            _ => {}
        }
    } else if app.privacy_confirmation.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
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
            KeyCode::Char('i' | 'I' | ' ') | KeyCode::Left | KeyCode::Right => {
                let prompt = app
                    .privacy_confirmation
                    .as_mut()
                    .expect("privacy confirmation open");
                prompt.allow_images = !prompt.allow_images;
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                app.privacy_confirmation = None;
                app.pending_privacy_message = None;
                app.notice = "Request cancelled; nothing was sent.".to_owned();
            }
            _ => {}
        }
    } else if app.chain_form.is_some() {
        app.handle_chain_form(key)?;
    } else if app.provider_form.is_some() {
        app.handle_provider_form(key)?;
    } else if app.model_choices.is_some() {
        let choices = app.model_choices.as_ref().expect("model choices open");
        match key.code {
            KeyCode::Up | KeyCode::Left => {
                app.model_choice_index = app.model_choice_index.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Right => {
                app.model_choice_index =
                    (app.model_choice_index + 1).min(choices.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                if let Some((provider_index, _, model)) =
                    choices.get(app.model_choice_index).cloned()
                {
                    app.activate_model(provider_index, &model)?;
                }
            }
            KeyCode::Esc => {
                app.model_choices = None;
                app.pending_model = None;
            }
            _ => {}
        }
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
        if let Some(edit) = editing::edit_key(key) {
            app.edit_input(edit);
            app.refresh_popups();
            return Ok(());
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let slash = app.slash.is_open() && app.mention.is_none();
        match key.code {
            KeyCode::Up if app.mention.is_some() && !control => app.mention_move(-1),
            KeyCode::Down if app.mention.is_some() && !control => app.mention_move(1),
            KeyCode::Tab if app.mention.is_some() => app.accept_mention(),
            KeyCode::Up if slash && !control => app.slash_move(-1),
            KeyCode::Down if slash && !control => app.slash_move(1),
            KeyCode::Tab if slash => app.accept_slash(),
            KeyCode::Esc if slash => app.dismiss_slash(),
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                app.insert_input("\n")
            }
            KeyCode::Enter if app.mention_takes_enter() => app.accept_mention(),
            KeyCode::Enter if slash && app.slash_takes_enter() => app.accept_slash(),
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
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.insert_input(character.encode_utf8(&mut [0; 4]));
            }
            _ => {}
        }
        app.refresh_popups();
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

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn arrows_move_the_cursor_and_typing_inserts_there() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        super::handle_batch(&mut app, &typed("fix bug")).unwrap();
        for _ in 0..3 {
            handle_key(&mut app, key_press(KeyCode::Left)).unwrap();
        }
        super::handle_batch(&mut app, &typed("the ")).unwrap();
        assert_eq!(app.input, "fix the bug");
        handle_key(&mut app, key_press(KeyCode::Delete)).unwrap();
        assert_eq!(app.input, "fix the ug");
        handle_key(&mut app, key_press(KeyCode::Backspace)).unwrap();
        assert_eq!(app.input, "fix theug");
        handle_key(&mut app, with(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        super::handle_batch(&mut app, &typed(">")).unwrap();
        handle_key(&mut app, with(KeyCode::Char('e'), KeyModifiers::CONTROL)).unwrap();
        super::handle_batch(&mut app, &typed("<")).unwrap();
        assert_eq!(app.input, ">fix theug<");
        handle_key(&mut app, with(KeyCode::Left, KeyModifiers::CONTROL)).unwrap();
        handle_key(&mut app, with(KeyCode::Backspace, KeyModifiers::CONTROL)).unwrap();
        assert_eq!(
            app.input, ">theug<",
            "Ctrl+Backspace deletes the word before"
        );
        handle_key(&mut app, with(KeyCode::Char('w'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(app.input, "theug<", "Ctrl+W deletes back to the space");
        assert!(app.running && app.transcript.is_empty(), "nothing was sent");
    }

    #[test]
    fn new_lines_and_pasted_lines_go_in_at_the_cursor() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.set_input("ab");
        handle_key(&mut app, key_press(KeyCode::Left)).unwrap();
        handle_key(&mut app, with(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.input, "a\nb");
        super::handle_batch(&mut app, &typed("x\ny")).unwrap();
        assert_eq!(app.input, "a\nx\nyb");
        assert!(app.transcript.is_empty());
    }

    #[test]
    fn multi_byte_text_is_never_split_by_the_keys() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        super::handle_batch(&mut app, &typed("привет 👍🏽 мир")).unwrap();
        handle_key(&mut app, with(KeyCode::Char('b'), KeyModifiers::ALT)).unwrap();
        handle_key(&mut app, key_press(KeyCode::Left)).unwrap();
        handle_key(&mut app, key_press(KeyCode::Backspace)).unwrap();
        assert_eq!(app.input, "привет  мир", "the emoji went in one piece");
        handle_key(&mut app, key_press(KeyCode::Home)).unwrap();
        handle_key(&mut app, with(KeyCode::Char('f'), KeyModifiers::ALT)).unwrap();
        handle_key(&mut app, key_press(KeyCode::Backspace)).unwrap();
        assert_eq!(app.input, "приве  мир");
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
