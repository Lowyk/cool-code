mod agent_bridge;
mod commands;
mod context;
mod effort;
mod forms;
mod models;
mod render;
mod state;
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

pub(crate) fn run() -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal);
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

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new(read_settings()?);
    let animation_start = std::time::Instant::now();
    while app.running {
        app.poll_response();
        let animation_tick = (animation_start.elapsed().as_millis() / 280) as usize;
        terminal.draw(|frame| draw(frame, &app, animation_tick))?;
        if !event::poll(Duration::from_millis(100)).context("waiting for terminal input")? {
            continue;
        }
        let Event::Key(key) = event::read().context("reading terminal input")? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

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
                        continue;
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
        } else if app.settings_menu {
            app.handle_settings_key(key)?;
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
        } else {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.running = false;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.toggle_chain()?
                }
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
    }
    Ok(())
}
