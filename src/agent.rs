use crate::policy::{auto_approve_command, auto_approve_create, auto_approve_edit, mode_label};
use crate::stream::{Stream, StreamEvent};
use crate::tools::ToolSet;
use crate::workflow::{Completer, ProviderCompleter, Run};
use crate::{Settings, provider};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender};

pub(crate) enum PendingEvent {
    TextDelta(String),
    Usage(u64),
    ToolStarted(String),
    ToolAction(String),
    ConversationMessage(provider::ChatMessage),
    ApprovalRequest(ToolApproval),
    Finished(std::result::Result<provider::Completion, String>),
}

pub(crate) struct ToolApproval {
    pub(crate) title: String,
    pub(crate) details: String,
    pub(crate) response: SyncSender<bool>,
}

/// Budget for one turn: (tool rounds, total tool calls), from the `max_tool_rounds` setting.
/// The budget only bounds runaway cost; permission modes still gate every edit and command.
pub(crate) fn tool_limits(settings: &Settings) -> (usize, usize) {
    let rounds = settings.max_tool_rounds.clamp(1, 200);
    (rounds, rounds * 4)
}

pub(crate) fn round_limit_message(rounds: usize) -> String {
    format!(
        "tool-call limit reached ({rounds} rounds) before a final response; send \"continue\" to keep going, or raise max_tool_rounds in ~/.coolcode/config.toml"
    )
}

/// Counters for one model attempt, kept for usage stats. A fallback attempt starts them over, so
/// a failed model's counts never land in the record of the model that finally answered.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tally {
    pub(crate) input: Option<u64>,
    pub(crate) output: Option<u64>,
    pub(crate) chars: usize,
}

impl Tally {
    pub(crate) fn observe(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::TextDelta(text) => self.chars += text.chars().count(),
            StreamEvent::Usage { input, output } => {
                self.input = input.or(self.input);
                self.output = output.or(self.output);
            }
            StreamEvent::Attempt => *self = Tally::default(),
        }
    }
}

/// Characters of text in a request, used only to estimate tokens a provider did not report.
pub(crate) fn message_chars(messages: &[provider::ChatMessage]) -> usize {
    messages
        .iter()
        .map(|message| match &message.content {
            serde_json::Value::String(text) => text.chars().count(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                .map(|text| text.chars().count())
                .sum(),
            _ => 0,
        })
        .sum()
}

/// Records one model request in the usage stats (when they are switched on).
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_request(
    settings: &Settings,
    turn_id: &str,
    started_ts: i64,
    timer: std::time::Instant,
    result: &Result<provider::Completion>,
    observed: Tally,
    input_chars: usize,
    cancel: &std::sync::atomic::AtomicBool,
) {
    if !settings.stats_enabled {
        return;
    }
    let (provider_id, model, tool_calls, outcome) = match result {
        Ok(completion) => (
            completion.provider_id.clone(),
            completion.model_id.clone(),
            completion.tool_calls.len() as u32,
            crate::stats::Outcome::Done,
        ),
        Err(error) => {
            let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed)
                || error
                    .downcast_ref::<crate::stream::Interrupted>()
                    .is_some_and(|interrupted| interrupted.reason == "cancelled");
            (
                settings.active_provider_id.clone(),
                settings.model.clone().unwrap_or_default(),
                0,
                if cancelled {
                    crate::stats::Outcome::Cancelled
                } else {
                    crate::stats::Outcome::Failed
                },
            )
        }
    };
    let provider_name = provider_id
        .as_deref()
        .and_then(|id| settings.providers.iter().find(|profile| profile.id == id))
        .map_or_else(|| "unknown".to_owned(), |profile| profile.name.clone());
    crate::stats::record(&crate::stats::build_record(&crate::stats::Facts {
        started_ts,
        turn: turn_id,
        provider: &provider_name,
        model: &model,
        reported_input: observed.input,
        reported_output: observed.output,
        input_chars,
        output_chars: observed.chars,
        duration_ms: timer.elapsed().as_millis() as u64,
        tool_calls,
        outcome,
    }));
}

/// The text of the user's most recent message, for telling a reviewer what was asked.
fn latest_user_text(messages: &[provider::ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.display.clone())
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_default()
}

pub(crate) fn run_agent_turns(
    settings: Settings,
    messages: Vec<provider::ChatMessage>,
    workspace_root: PathBuf,
    workspace_trusted: bool,
    events: &mpsc::Sender<PendingEvent>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<provider::Completion> {
    let completer = ProviderCompleter(&settings);
    run_loop(
        &completer,
        &settings,
        messages,
        &workspace_root,
        workspace_trusted,
        events,
        &cancel,
    )
}

/// The assistant's turn: ask the model, run the tools it calls, and repeat until it answers.
/// While workflows are on it can also delegate to subagents, and its finished work is reviewed.
pub(crate) fn run_loop(
    completer: &dyn Completer,
    settings: &Settings,
    mut messages: Vec<provider::ChatMessage>,
    workspace_root: &Path,
    workspace_trusted: bool,
    events: &mpsc::Sender<PendingEvent>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<provider::Completion> {
    let (max_rounds, max_calls) = tool_limits(settings);
    let mut calls_run = 0usize;
    let mut approved_plan: Vec<(String, serde_json::Value)> = Vec::new();
    let mut flow = Run::new(settings);
    let request_text = latest_user_text(&messages);
    let tools = if workspace_trusted {
        ToolSet::Main {
            plan_mode: settings.permission_mode == "plan",
            workflows: flow.enabled(),
        }
    } else {
        ToolSet::None
    };
    let turn_id = uuid::Uuid::new_v4().simple().to_string();
    let tally = std::cell::RefCell::new(Tally::default());
    let forward = |event| {
        tally.borrow_mut().observe(&event);
        let pending = match event {
            StreamEvent::TextDelta(text) => PendingEvent::TextDelta(text),
            StreamEvent::Usage {
                output: Some(tokens),
                ..
            } => PendingEvent::Usage(tokens),
            StreamEvent::Usage { output: None, .. } | StreamEvent::Attempt => return,
        };
        let _ = events.send(pending);
    };
    let stream = Stream {
        on_event: &forward,
        cancel,
    };
    for _ in 0..=max_rounds {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("cancelled");
        }
        *tally.borrow_mut() = Tally::default();
        let started_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64);
        let timer = std::time::Instant::now();
        let result = completer.complete(&messages, tools, &stream);
        record_request(
            settings,
            &turn_id,
            started_ts,
            timer,
            &result,
            *tally.borrow(),
            message_chars(&messages),
            cancel,
        );
        let mut completion = result?;
        if completion.tool_calls.is_empty() {
            if let Some(report) = flow.review_after_final(
                completer,
                settings,
                workspace_root,
                &request_text,
                events,
                cancel,
            )? {
                // The reviewer found real problems: hand them back for a fix round.
                let said = provider::ChatMessage::assistant(completion.text.clone());
                messages.push(said.clone());
                let _ = events.send(PendingEvent::ConversationMessage(said));
                let feedback = format!(
                    "[Automatic review by a separate reviewer]\n{report}\n\nFix the real problems it found, or explain why one does not apply, then finish."
                );
                let note =
                    provider::ChatMessage::user_with_images(feedback.clone(), feedback, Vec::new());
                messages.push(note.clone());
                let _ = events.send(PendingEvent::ConversationMessage(note));
                continue;
            }
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
        if completion.tool_calls.len() > 4 || calls_run + completion.tool_calls.len() > max_calls {
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
        let signatures = completion
            .tool_calls
            .iter()
            .filter_map(|call| Some((call.id.clone(), call.thought_signature.clone()?)))
            .collect();
        let assistant_tool_message =
            provider::ChatMessage::assistant_tool_calls(completion.text, wire_calls)
                .with_thought_signatures(signatures);
        messages.push(assistant_tool_message.clone());
        let _ = events.send(PendingEvent::ConversationMessage(assistant_tool_message));
        for call in completion.tool_calls {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                bail!("cancelled");
            }
            calls_run += 1;
            let label = call
                .arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .filter(|_| call.name == "run_command")
                .unwrap_or(&call.name)
                .to_owned();
            let _ = events.send(PendingEvent::ToolStarted(label));
            let outcome = if call.name == "spawn_subagents" && flow.enabled() {
                flow.spawn(
                    completer,
                    settings,
                    workspace_root,
                    &call.arguments,
                    events,
                    cancel,
                )
            } else {
                execute_agent_tool(
                    settings,
                    workspace_root,
                    &call.name,
                    &call.arguments,
                    events,
                    &mut approved_plan,
                    cancel,
                    None,
                )
            };
            // A cancel ends the turn; any other failure is just the tool's answer.
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                bail!("cancelled");
            }
            let result = outcome.unwrap_or_else(|error| format!("Tool error: {error:#}"));
            flow.note_tool(&call.name, &result);
            let summary = summarize_tool_result(&result);
            let action = format!("Tool · {} · {summary}", call.name);
            let _ = events.send(PendingEvent::ToolAction(action));
            let tool_message = provider::ChatMessage::tool_result(call.id, call.name, result);
            messages.push(tool_message.clone());
            let _ = events.send(PendingEvent::ConversationMessage(tool_message));
        }
    }
    bail!(round_limit_message(max_rounds))
}

/// Prefixes an approval title with the subagent that asked, so the user can tell who is acting.
fn titled(actor: Option<&str>, title: String) -> String {
    match actor {
        Some(actor) => format!("Subagent ({actor}): {title}"),
        None => title,
    }
}

/// Runs one tool call under the permission rules. `actor` names a subagent making the call.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_agent_tool(
    settings: &Settings,
    root: &Path,
    name: &str,
    arguments: &serde_json::Value,
    events: &mpsc::Sender<PendingEvent>,
    approved_plan: &mut Vec<(String, serde_json::Value)>,
    cancel: &std::sync::atomic::AtomicBool,
    actor: Option<&str>,
) -> Result<String> {
    if name == "request_plan_approval" {
        return request_plan_approval(settings, root, arguments, events, approved_plan);
    }
    // Plan mode gates only what changes things; reading and searching are always allowed.
    let changes_things = matches!(
        name,
        "replace_text" | "replace_in_file" | "write_to_file" | "create_file" | "run_command"
    );
    let planned = if settings.permission_mode == "plan" && changes_things {
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
            if !request_tool_approval(
                events,
                titled(actor, "Run shell command".to_owned()),
                details,
            )? {
                return Ok(
                    "The user declined this command. Do not retry without new authorization."
                        .to_owned(),
                );
            }
        }
        return crate::tools::run_command(root, command, cancel);
    }
    if !matches!(
        name,
        "replace_in_file" | "replace_text" | "write_to_file" | "create_file"
    ) {
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
                titled(actor, format!("Create {}", proposal.relative_path)),
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

    let proposal = if name == "replace_text" {
        if object
            .keys()
            .any(|key| !["path", "old_text", "new_text", "replace_all"].contains(&key.as_str()))
        {
            bail!("replace_text received an unknown argument");
        }
        let replace_all = match arguments.get("replace_all") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(value)) => *value,
            Some(_) => bail!("replace_text `replace_all` must be true or false"),
        };
        crate::tools::prepare_replace_text(
            root,
            path,
            required("old_text")?,
            required_allowing_empty(arguments, "new_text", name)?,
            replace_all,
        )?
    } else if name == "replace_in_file" {
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
        if !request_tool_approval(
            events,
            titled(actor, format!("Edit {}", proposal.relative_path)),
            details,
        )? {
            return Ok("The user declined this proposed edit. Do not retry it without changing the proposal or receiving new authorization.".to_owned());
        }
    }
    crate::tools::apply_edit(root, &proposal)?;
    Ok(format!(
        "Updated {} ({}).",
        proposal.relative_path, proposal.change_summary
    ))
}

/// A string argument that may be empty (an empty `new_text` deletes the matched text).
fn required_allowing_empty<'a>(
    arguments: &'a serde_json::Value,
    key: &str,
    tool: &str,
) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_str)
        .context(format!("{tool} requires a string `{key}`"))
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
            "replace_text" | "replace_in_file" | "write_to_file" | "create_file" | "run_command"
        ) {
            bail!("plans may include only file edits/creation and run_command actions");
        }
        if name == "replace_text" {
            let text = |key: &str| -> Result<&str> {
                args.get(key)
                    .and_then(serde_json::Value::as_str)
                    .context(format!("planned replace_text requires `{key}`"))
            };
            let replace_all = args
                .get("replace_all")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let proposal = crate::tools::prepare_replace_text(
                root,
                text("path")?,
                text("old_text")?,
                text("new_text")?,
                replace_all,
            )?;
            lines.push(format!("{}. Edit:\n{}", index + 1, proposal.preview()));
        } else if name == "replace_in_file" {
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

pub(crate) fn summarize_tool_result(result: &str) -> String {
    let one_line = result.lines().take(2).collect::<Vec<_>>().join(" · ");
    let shortened = one_line.chars().take(180).collect::<String>();
    if result.lines().count() > 2 || result.chars().count() > 180 {
        format!("{shortened}…")
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::{message_chars, round_limit_message, tool_limits};
    use crate::Settings;

    #[test]
    fn message_chars_counts_text_but_not_images() {
        use crate::provider::ChatMessage;
        let text = ChatMessage::system("abcd".to_owned());
        let mut with_image = ChatMessage::system(String::new());
        with_image.content = serde_json::json!([
            {"type": "text", "text": "hello"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAAAAAAAAAAAAAAAAAA"}}
        ]);
        assert_eq!(message_chars(&[text, with_image]), 9);
        assert_eq!(message_chars(&[]), 0);
    }

    #[test]
    fn the_tally_counts_text_and_keeps_the_latest_counts() {
        use super::Tally;
        use crate::stream::StreamEvent;
        let mut tally = Tally::default();
        tally.observe(&StreamEvent::TextDelta("héllo".to_owned()));
        tally.observe(&StreamEvent::Usage {
            input: Some(10),
            output: Some(1),
        });
        tally.observe(&StreamEvent::Usage {
            input: None,
            output: Some(8),
        });
        assert_eq!(
            tally,
            Tally {
                input: Some(10),
                output: Some(8),
                chars: 5
            }
        );
    }

    #[test]
    fn a_new_attempt_starts_the_tally_over() {
        use super::Tally;
        use crate::stream::StreamEvent;
        let mut tally = Tally::default();
        tally.observe(&StreamEvent::Usage {
            input: Some(500),
            output: None,
        });
        tally.observe(&StreamEvent::Attempt);
        tally.observe(&StreamEvent::Usage {
            input: None,
            output: Some(7),
        });
        assert_eq!(
            tally.input, None,
            "the failed attempt's input must not carry over"
        );
        assert_eq!(tally.output, Some(7));
    }

    fn plan_mode_workspace() -> (Settings, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("harness-agent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "hello\nworld\n").unwrap();
        let mut settings = Settings::default();
        settings.permission_mode = "plan".to_owned();
        (settings, root)
    }

    #[test]
    fn plan_mode_lets_the_model_read_but_not_change_things() {
        let (settings, root) = plan_mode_workspace();
        let (events, _received) = std::sync::mpsc::channel();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let run = |name: &str, arguments: serde_json::Value| {
            super::execute_agent_tool(
                &settings,
                &root,
                name,
                &arguments,
                &events,
                &mut Vec::new(),
                &cancel,
                None,
            )
            // A tool error (such as git_status outside a repository) is still an answer.
            .unwrap_or_else(|error| format!("{error:#}"))
        };
        for (name, arguments) in [
            ("list_files", serde_json::json!({})),
            ("read_file", serde_json::json!({"path": "a.txt"})),
            ("search_text", serde_json::json!({"query": "world"})),
            ("git_status", serde_json::json!({})),
        ] {
            let result = run(name, arguments);
            assert!(
                !result.contains("Plan mode blocks"),
                "{name} is read-only and must work in Plan mode: {result}"
            );
        }
        assert!(run("read_file", serde_json::json!({"path": "a.txt"})).contains("hello"));
        for (name, arguments) in [
            (
                "replace_text",
                serde_json::json!({"path": "a.txt", "old_text": "hello", "new_text": "bye"}),
            ),
            (
                "create_file",
                serde_json::json!({"path": "b.txt", "content": "x"}),
            ),
            ("run_command", serde_json::json!({"command": "echo hi"})),
        ] {
            assert!(
                run(name, arguments).contains("Plan mode blocks"),
                "{name} changes things and needs an approved plan"
            );
        }
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "hello\nworld\n"
        );
        assert!(!root.join("b.txt").exists());
    }

    #[test]
    fn default_budget_fits_building_an_app() {
        assert_eq!(tool_limits(&Settings::default()), (40, 160));
    }

    #[test]
    fn budget_is_clamped_to_a_sane_range() {
        let mut settings = Settings::default();
        settings.max_tool_rounds = 0;
        assert_eq!(tool_limits(&settings), (1, 4));
        settings.max_tool_rounds = 100_000;
        assert_eq!(tool_limits(&settings), (200, 800));
    }

    #[test]
    fn limit_message_says_how_to_continue() {
        let message = round_limit_message(40);
        assert!(message.contains("40"), "{message}");
        assert!(message.contains("continue"), "{message}");
        assert!(message.contains("max_tool_rounds"), "{message}");
    }
}
