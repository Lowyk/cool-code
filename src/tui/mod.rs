mod commands;
mod context;
mod effort;
mod forms;
mod models;
mod render;
mod state;
mod wordmark;

use crate::tui::render::draw;
use crate::tui::state::{
    App, LEVELS, MODES, PendingEvent, ToolApproval, TranscriptEntry, TranscriptKind, mode_label,
};
use crate::{Effort, Settings, provider, read_settings, write_settings};
use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

impl App {}

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

fn run_agent_turns(
    settings: Settings,
    mut messages: Vec<provider::ChatMessage>,
    workspace_root: PathBuf,
    workspace_trusted: bool,
    events: &mpsc::Sender<PendingEvent>,
) -> Result<provider::Completion> {
    const MAX_TOOL_ROUNDS: usize = 6;
    const MAX_TOOL_CALLS: usize = 16;
    let mut calls_run = 0usize;
    let mut approved_plan: Vec<(String, serde_json::Value)> = Vec::new();
    for _ in 0..=MAX_TOOL_ROUNDS {
        let mut completion =
            provider::complete_with_fallback(&settings, &messages, workspace_trusted)?;
        if completion.tool_calls.is_empty() {
            if !approved_plan.is_empty() {
                completion.text.push_str(&format!(
                    "\n\nNote: the approved plan ended with {} action(s) not yet performed.",
                    approved_plan.len()
                ));
            }
            return Ok(completion);
        }
        if !workspace_trusted {
            bail!("provider requested repository tools, but this workspace is not trusted");
        }
        if completion.tool_calls.len() > 4
            || calls_run + completion.tool_calls.len() > MAX_TOOL_CALLS
        {
            bail!("tool-call budget exceeded; stopping this agent turn safely");
        }
        let wire_calls = completion
            .tool_calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                    }
                })
            })
            .collect();
        let assistant_tool_message =
            provider::ChatMessage::assistant_tool_calls(completion.text, wire_calls);
        messages.push(assistant_tool_message.clone());
        let _ = events.send(PendingEvent::ConversationMessage(assistant_tool_message));
        for call in completion.tool_calls {
            calls_run += 1;
            let result = execute_agent_tool(
                &settings,
                &workspace_root,
                &call.name,
                &call.arguments,
                events,
                &mut approved_plan,
            )
            .unwrap_or_else(|error| format!("Tool error: {error:#}"));
            let summary = summarize_tool_result(&result);
            let action = format!("Tool · {} · {summary}", call.name);
            let _ = events.send(PendingEvent::ToolAction(action));
            let tool_message = provider::ChatMessage::tool_result(call.id, call.name, result);
            messages.push(tool_message.clone());
            let _ = events.send(PendingEvent::ConversationMessage(tool_message));
        }
    }
    bail!("agent tool-call round limit reached without a final response")
}

fn execute_agent_tool(
    settings: &Settings,
    root: &Path,
    name: &str,
    arguments: &serde_json::Value,
    events: &mpsc::Sender<PendingEvent>,
    approved_plan: &mut Vec<(String, serde_json::Value)>,
) -> Result<String> {
    if name == "request_plan_approval" {
        return request_plan_approval(settings, root, arguments, events, approved_plan);
    }
    let planned = if settings.permission_mode == "plan" {
        if approved_plan
            .first()
            .is_some_and(|(planned_name, planned_args)| {
                planned_name == name && planned_args == arguments
            })
        {
            approved_plan.remove(0);
            true
        } else {
            return Ok(
                "Plan mode blocks this action: request approval for it first, and carry out approved actions in the exact order and with the exact arguments shown.".to_owned(),
            );
        }
    } else {
        false
    };
    if name == "run_command" {
        let object = arguments
            .as_object()
            .context("tool arguments must be an object")?;
        if object.keys().any(|key| key != "command") {
            bail!("run_command received an unknown argument");
        }
        let command = arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .context("run_command requires a string `command`")?;
        if !planned && !auto_approve_command(&settings.permission_mode, command) {
            let details = format!(
                "Permission mode: {}\nWorking directory: {}\nShell: {}\n\nExact command to execute:\n{}",
                mode_label(&settings.permission_mode),
                root.display(),
                if cfg!(windows) {
                    "PowerShell (no profile)"
                } else {
                    "POSIX sh -lc"
                },
                command,
            );
            if !request_tool_approval(events, "Run shell command".to_owned(), details)? {
                return Ok(
                    "The user declined this command. Do not retry without new authorization."
                        .to_owned(),
                );
            }
        }
        return crate::tools::run_command(root, command);
    }
    if !matches!(name, "replace_in_file" | "write_to_file" | "create_file") {
        return crate::tools::execute_read_only(root, name, arguments);
    }
    let object = arguments
        .as_object()
        .context("tool arguments must be an object")?;
    let required = |key: &str| -> Result<&str> {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .context(format!("{name} requires a string `{key}`"))
    };
    let path = required("path")?;
    if name == "create_file" {
        if object
            .keys()
            .any(|key| !["path", "content"].contains(&key.as_str()))
        {
            bail!("create_file received an unknown argument");
        }
        let proposal = crate::tools::prepare_create_file(root, path, required("content")?)?;
        if !planned && !auto_approve_create(&settings.permission_mode, &proposal) {
            let details = format!(
                "Permission mode: {}\n\nProposed complete new-file contents:\n{}",
                mode_label(&settings.permission_mode),
                proposal.preview(),
            );
            if !request_tool_approval(
                events,
                format!("Create {}", proposal.relative_path),
                details,
            )? {
                return Ok(
                    "The user declined this file creation. Do not retry without new authorization."
                        .to_owned(),
                );
            }
        }
        crate::tools::apply_create(root, &proposal)?;
        return Ok(format!("Created new file {}.", proposal.relative_path));
    }

    let proposal = if name == "replace_in_file" {
        if object.keys().any(|key| {
            ![
                "path",
                "start_line",
                "end_line",
                "expected_text",
                "replacement",
            ]
            .contains(&key.as_str())
        }) {
            bail!("replace_in_file received an unknown argument");
        }
        let line = |key: &str| -> Result<usize> {
            arguments
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .and_then(|line| usize::try_from(line).ok())
                .context(format!(
                    "replace_in_file requires a non-negative integer `{key}`"
                ))
        };
        crate::tools::prepare_replace_lines(
            root,
            path,
            line("start_line")?,
            line("end_line")?,
            required("expected_text")?,
            required("replacement")?,
        )?
    } else {
        if object
            .keys()
            .any(|key| !["path", "line_number", "text"].contains(&key.as_str()))
        {
            bail!("write_to_file received an unknown argument");
        }
        let line_number = arguments
            .get("line_number")
            .and_then(serde_json::Value::as_u64)
            .and_then(|line| usize::try_from(line).ok())
            .context("write_to_file requires a non-negative integer `line_number`")?;
        crate::tools::prepare_write_to_file(root, path, line_number, required("text")?)?
    };
    if !planned && !auto_approve_edit(&settings.permission_mode, &proposal) {
        let details = format!(
            "Permission mode: {}\n\nProposed file change:\n{}",
            mode_label(&settings.permission_mode),
            proposal.preview(),
        );
        if !request_tool_approval(events, format!("Edit {}", proposal.relative_path), details)? {
            return Ok("The user declined this proposed edit. Do not retry it without changing the proposal or receiving new authorization.".to_owned());
        }
    }
    crate::tools::apply_edit(root, &proposal)?;
    Ok(format!(
        "Updated {} ({}).",
        proposal.relative_path, proposal.change_summary
    ))
}

fn auto_approve_create(permission_mode: &str, proposal: &crate::tools::CreateProposal) -> bool {
    match permission_mode {
        "accept-everything" | "accept-edits" | "accept-minimal" => true,
        "auto" => {
            let path = proposal
                .relative_path
                .replace('\\', "/")
                .to_ascii_lowercase();
            proposal.is_small()
                && !path.split('/').any(|part| {
                    part.starts_with(".env")
                        || part.contains("secret")
                        || part.contains("credential")
                })
        }
        _ => false,
    }
}

fn request_plan_approval(
    settings: &Settings,
    root: &Path,
    arguments: &serde_json::Value,
    events: &mpsc::Sender<PendingEvent>,
    approved_plan: &mut Vec<(String, serde_json::Value)>,
) -> Result<String> {
    if settings.permission_mode != "plan" {
        return Ok("request_plan_approval is available only in Plan mode.".to_owned());
    }
    if !approved_plan.is_empty() {
        return Ok("An approved plan still has unfinished actions; complete those before requesting another plan.".to_owned());
    }
    let object = arguments
        .as_object()
        .context("plan arguments must be an object")?;
    if object
        .keys()
        .any(|key| !["summary", "actions"].contains(&key.as_str()))
    {
        bail!("request_plan_approval received an unknown argument");
    }
    let summary = arguments
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .context("plan summary must be a non-empty string")?;
    let actions = arguments
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .context("plan actions must be an array")?;
    if actions.is_empty() || actions.len() > 8 {
        bail!("a plan must contain between 1 and 8 edit/command actions");
    }
    let mut planned = Vec::new();
    let mut lines = vec![format!("Plan: {summary}"), String::new()];
    for (index, action) in actions.iter().enumerate() {
        let name = action
            .get("name")
            .and_then(serde_json::Value::as_str)
            .context("each plan action needs a tool name")?;
        let args = action
            .get("arguments")
            .filter(|value| value.is_object())
            .context("each plan action needs an arguments object")?;
        if !matches!(
            name,
            "replace_in_file" | "write_to_file" | "create_file" | "run_command"
        ) {
            bail!("plans may include only file edits/creation and run_command actions");
        }
        if name == "replace_in_file" {
            let path = args
                .get("path")
                .and_then(serde_json::Value::as_str)
                .context("planned edit requires a path")?;
            let start_line = args
                .get("start_line")
                .and_then(serde_json::Value::as_u64)
                .and_then(|line| usize::try_from(line).ok())
                .context("planned edit requires start_line")?;
            let end_line = args
                .get("end_line")
                .and_then(serde_json::Value::as_u64)
                .and_then(|line| usize::try_from(line).ok())
                .context("planned edit requires end_line")?;
            let expected_text = args
                .get("expected_text")
                .and_then(serde_json::Value::as_str)
                .context("planned edit requires expected_text")?;
            let replacement = args
                .get("replacement")
                .and_then(serde_json::Value::as_str)
                .context("planned edit requires replacement text")?;
            let proposal = crate::tools::prepare_replace_lines(
                root,
                path,
                start_line,
                end_line,
                expected_text,
                replacement,
            )?;
            lines.push(format!("{}. Edit:\n{}", index + 1, proposal.preview()));
        } else if name == "write_to_file" {
            let path = args
                .get("path")
                .and_then(serde_json::Value::as_str)
                .context("planned insertion requires a path")?;
            let line_number = args
                .get("line_number")
                .and_then(serde_json::Value::as_u64)
                .and_then(|line| usize::try_from(line).ok())
                .context("planned insertion requires line_number")?;
            let text = args
                .get("text")
                .and_then(serde_json::Value::as_str)
                .context("planned insertion requires text")?;
            let proposal = crate::tools::prepare_write_to_file(root, path, line_number, text)?;
            lines.push(format!("{}. Insert:\n{}", index + 1, proposal.preview()));
        } else if name == "create_file" {
            let path = args
                .get("path")
                .and_then(serde_json::Value::as_str)
                .context("planned creation requires a path")?;
            let content = args
                .get("content")
                .and_then(serde_json::Value::as_str)
                .context("planned creation requires complete content")?;
            let proposal = crate::tools::prepare_create_file(root, path, content)?;
            lines.push(format!("{}. Create:\n{}", index + 1, proposal.preview()));
        } else {
            let command = args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .context("planned command is required")?;
            if command.trim().is_empty() || command.len() > 8 * 1024 {
                bail!("planned command is empty or exceeds 8 KiB");
            }
            lines.push(format!(
                "{}. Command in {}:\n{}",
                index + 1,
                root.display(),
                command
            ));
        }
        planned.push((name.to_owned(), args.clone()));
    }
    if request_tool_approval(
        events,
        "Approve execution plan".to_owned(),
        lines.join("\n\n"),
    )? {
        *approved_plan = planned;
        Ok(format!(
            "The user approved this exact plan with {} action(s). Perform only those actions.",
            approved_plan.len()
        ))
    } else {
        Ok("The user declined the plan. Do not perform its edits or commands.".to_owned())
    }
}

fn auto_approve_edit(permission_mode: &str, proposal: &crate::tools::EditProposal) -> bool {
    match permission_mode {
        "accept-everything" | "accept-edits" => true,
        "accept-minimal" => true,
        "auto" => {
            let path = proposal
                .relative_path
                .replace('\\', "/")
                .to_ascii_lowercase();
            proposal.is_small()
                && !path.split('/').any(|part| {
                    part.starts_with(".env")
                        || part.contains("secret")
                        || part.contains("credential")
                })
        }
        _ => false,
    }
}

fn auto_approve_command(permission_mode: &str, command: &str) -> bool {
    if permission_mode == "accept-everything" {
        return true;
    }
    if !matches!(permission_mode, "auto" | "accept-minimal") {
        return false;
    }
    let normalized = command.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "cargo fmt --check"
            | "cargo check"
            | "cargo test"
            | "npm test"
            | "npm run build"
            | "pytest"
    )
}

fn request_tool_approval(
    events: &mpsc::Sender<PendingEvent>,
    title: String,
    details: String,
) -> Result<bool> {
    let (response, wait) = mpsc::sync_channel(1);
    events
        .send(PendingEvent::ApprovalRequest(ToolApproval {
            title,
            details,
            response,
        }))
        .context("sending action approval request to the TUI")?;
    wait.recv()
        .context("waiting for the action approval decision")
}

fn summarize_tool_result(result: &str) -> String {
    let one_line = result.lines().take(2).collect::<Vec<_>>().join(" · ");
    let shortened = one_line.chars().take(180).collect::<String>();
    if result.lines().count() > 2 || result.chars().count() > 180 {
        format!("{shortened}…")
    } else {
        shortened
    }
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

#[cfg(test)]
mod tests {
    use super::{auto_approve_command, auto_approve_edit};

    #[test]
    fn permission_modes_have_deterministic_edit_and_command_rules() {
        let proposal = crate::tools::EditProposal {
            relative_path: "src/main.rs".to_owned(),
            original: "old".to_owned(),
            updated: "new".to_owned(),
            change_summary: "Replace lines 1-1".to_owned(),
            before: "old".to_owned(),
            after: "new".to_owned(),
        };
        assert!(!auto_approve_edit("plan", &proposal));
        assert!(auto_approve_edit("accept-edits", &proposal));
        assert!(auto_approve_edit("accept-minimal", &proposal));
        assert!(auto_approve_edit("auto", &proposal));
        assert!(auto_approve_edit("accept-everything", &proposal));
        assert!(!auto_approve_command("plan", "cargo test"));
        assert!(!auto_approve_command("accept-edits", "cargo test"));
        assert!(auto_approve_command("accept-minimal", "cargo test"));
        assert!(!auto_approve_command(
            "accept-minimal",
            "cargo test; del important.txt"
        ));
        assert!(auto_approve_command("auto", "npm test"));
        assert!(auto_approve_command(
            "accept-everything",
            "Remove-Item -Recurse ."
        ));
    }
}
