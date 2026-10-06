use crate::policy::{auto_approve_command, auto_approve_create, auto_approve_edit};
use crate::tui::state::{PendingEvent, ToolApproval, mode_label};
use crate::{Settings, provider};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub(super) fn run_agent_turns(
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

pub(super) fn execute_agent_tool(
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

pub(super) fn request_plan_approval(
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

pub(super) fn request_tool_approval(
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

pub(super) fn summarize_tool_result(result: &str) -> String {
    let one_line = result.lines().take(2).collect::<Vec<_>>().join(" · ");
    let shortened = one_line.chars().take(180).collect::<String>();
    if result.lines().count() > 2 || result.chars().count() > 180 {
        format!("{shortened}…")
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {}
