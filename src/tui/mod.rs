mod backdrop;
mod commands;
mod context;
mod creators;
mod effort;
mod forms;
mod models;
mod pickers;
mod render;
mod series;
pub(crate) mod sessions;
mod settings;
mod state;
mod stats_view;
mod widgets;
mod wordmark;

use crate::policy::MODES;
use crate::tui::render::draw;
use crate::tui::state::{App, LEVELS, TranscriptEntry, TranscriptKind};
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
    app.motion_prompt = !app.settings.motion_prompt_answered;
    app.stats_prompt = !app.settings.stats_prompt_answered;
    let animation_start = std::time::Instant::now();
    while app.running {
        app.poll_response();
        let animation_tick = (animation_start.elapsed().as_millis() / 280) as usize;
        terminal.draw(|frame| draw(frame, &app, animation_tick))?;
        // Redraw faster while the effort picker animates or a response is streaming.
        let frame_interval = Duration::from_millis(if app.picker || app.pending.is_some() {
            40
        } else {
            100
        });
        if !event::poll(frame_interval).context("waiting for terminal input")? {
            continue;
        }
        let Event::Key(key) = event::read().context("reading terminal input")? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        handle_key(&mut app, key)?;
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
    } else if app.motion_prompt {
        match key.code {
            KeyCode::Left => app.motion_choice = 0,
            KeyCode::Right => app.motion_choice = 1,
            KeyCode::Enter => app.answer_motion_prompt(app.motion_choice == 1)?,
            KeyCode::Char('r' | 'R') => app.answer_motion_prompt(true)?,
            KeyCode::Char('k' | 'K') | KeyCode::Esc => app.answer_motion_prompt(false)?,
            _ => {}
        }
    } else if app.stats_prompt {
        match key.code {
            KeyCode::Left => app.stats_choice = 0,
            KeyCode::Right => app.stats_choice = 1,
            KeyCode::Enter => app.answer_stats_prompt(app.stats_choice == 0)?,
            KeyCode::Char('y' | 'Y') => app.answer_stats_prompt(true)?,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => app.answer_stats_prompt(false)?,
            _ => {}
        }
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
    } else if app.confirm_extreme {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                app.settings.extreme_acknowledged = true;
                app.apply_effort(Effort::Extreme)?;
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                app.confirm_extreme = false;
                app.notice = "Extreme was not selected.".to_owned();
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
        match key.code {
            KeyCode::Left => app.picker_index = app.picker_index.saturating_sub(1),
            KeyCode::Right => app.picker_index = (app.picker_index + 1).min(LEVELS.len() - 1),
            KeyCode::Enter => app.choose_effort()?,
            KeyCode::Esc => {
                app.picker = false;
                app.notice = "Effort unchanged.".to_owned();
            }
            _ => {}
        }
    } else if app.stats_view.is_some() {
        app.handle_stats_key(key)?;
    } else if app.settings_view.is_some() {
        app.handle_settings_view_key(key)?;
    } else {
        match key.code {
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

    #[test]
    fn motion_prompt_reduce_motion_disables_effects_and_is_answered_once() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.motion_prompt = true;
        handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)).expect("right");
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).expect("enter");
        assert!(!app.motion_prompt);
        assert!(app.settings.motion_prompt_answered);
        assert_eq!(app.settings.pulse, crate::PulseMode::Off);
        assert!(!app.settings.background_animation);
    }

    #[test]
    fn motion_prompt_keep_animations_leaves_effects_on() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.motion_prompt = true;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).expect("enter");
        assert!(!app.motion_prompt);
        assert!(app.settings.motion_prompt_answered);
        assert_eq!(app.settings.pulse, crate::PulseMode::Words);
        assert!(app.settings.background_animation);
        assert!(app.running);
    }

    fn stats_prompt_app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.stats_prompt = true;
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    }

    #[test]
    fn the_stats_prompt_defaults_to_no_and_enter_declines() {
        let mut app = stats_prompt_app();
        press(&mut app, KeyCode::Enter);
        assert!(!app.stats_prompt);
        assert!(!app.settings.stats_enabled);
        assert!(app.settings.stats_prompt_answered);
    }

    #[test]
    fn choosing_yes_in_the_stats_prompt_turns_recording_on() {
        let mut app = stats_prompt_app();
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.stats_enabled && app.settings.stats_prompt_answered);
        let mut shortcut = stats_prompt_app();
        press(&mut shortcut, KeyCode::Char('y'));
        assert!(shortcut.settings.stats_enabled);
        let mut declined = stats_prompt_app();
        press(&mut declined, KeyCode::Esc);
        assert!(!declined.settings.stats_enabled && declined.settings.stats_prompt_answered);
        assert!(
            declined.running,
            "Esc answers the prompt instead of quitting"
        );
    }

    #[test]
    fn the_stats_prompt_waits_for_the_motion_prompt() {
        let mut app = stats_prompt_app();
        app.motion_prompt = true;
        press(&mut app, KeyCode::Char('y'));
        // The key went to the motion prompt, not the stats prompt.
        assert!(app.stats_prompt && !app.settings.stats_enabled);
    }

    #[test]
    fn the_stats_prompt_explains_what_is_stored() {
        let app = stats_prompt_app();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 36)).expect("terminal");
        terminal
            .draw(|frame| crate::tui::render::draw(frame, &app, 0))
            .expect("draw");
        let shown: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(shown.contains("usage stats"), "{shown}");
        assert!(shown.contains("never your prompts"), "{shown}");
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
