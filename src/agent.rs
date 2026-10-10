use crate::policy::{auto_approve_command, auto_approve_file_change, mode_label};
use crate::stream::{Stream, StreamEvent};
use crate::tools::ToolSet;
use crate::workflow::{Completer, ProviderCompleter, Run};
use crate::{Settings, provider};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender};

/// What a file held after the model wrote it: its text, or (for a binary file such as an
/// image) a hash that recognizes it later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileContent {
    Text(String),
    Binary(String),
}

pub(crate) enum PendingEvent {
    TextDelta(String),
    Usage(u64),
    ToolStarted(String),
    ToolAction(String),
    ConversationMessage(provider::ChatMessage),
    /// The older part of the conversation was replaced by a summary: the first `replaced`
    /// messages (not counting the system prompt) become `with`.
    Compacted {
        replaced: usize,
        with: Vec<provider::ChatMessage>,
        summary: String,
    },
    /// A file was created or changed by an edit tool (for `/undo`).
    FileChanged {
        path: std::path::PathBuf,
        name: String,
        before: Option<String>,
        after: FileContent,
    },
    /// A `/compact` finished (or failed); there is no answer to show.
    CompactFinished(std::result::Result<(), String>),
    ApprovalRequest(ToolApproval),
    /// Progress of one workflow subagent, for the tracker.
    Subagent(crate::workflow::SubagentEvent),
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
            images: crate::imagegen::available(settings),
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
    // Condensing again is pointless once it stopped helping, and a provider's "too large" is
    // answered at most twice in one turn.
    let mut compaction_helps = true;
    let mut overflow_retries = 0usize;
    for _ in 0..=max_rounds {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("cancelled");
        }
        if compaction_helps && crate::context::should_compact(settings, &messages) {
            match crate::context::compact(completer, &messages, &stream) {
                Ok(compaction) => {
                    compaction_helps = compaction.tokens_after * 10 < compaction.tokens_before * 9;
                    announce_compaction(events, &compaction);
                    messages = compaction.messages;
                }
                // A failed summary must not stop the turn; the request is simply sent as is.
                Err(_) => compaction_helps = false,
            }
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
        if let Err(error) = &result
            && overflow_retries < 2
            && crate::context::is_context_overflow(&format!("{error:#}"))
            && let Ok(compaction) = crate::context::compact(completer, &messages, &stream)
        {
            overflow_retries += 1;
            announce_compaction(events, &compaction);
            messages = compaction.messages;
            continue;
        }
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
                    &crate::guard::GuardChain::from_settings(settings, request_text.clone()),
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

/// Tells the interface that older messages were replaced, so it keeps the same conversation.
pub(crate) fn announce_compaction(
    events: &mpsc::Sender<PendingEvent>,
    compaction: &crate::context::Compaction,
) {
    let _ = events.send(PendingEvent::Compacted {
        replaced: compaction.replaced,
        with: compaction.with.clone(),
        summary: crate::context::describe(compaction),
    });
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
    judge: &dyn crate::guard::Judge,
) -> Result<String> {
    if name == "request_plan_approval" {
        return request_plan_approval(settings, root, arguments, events, approved_plan);
    }
    // Plan mode gates only what changes things; reading and searching are always allowed.
    let changes_things = matches!(
        name,
        "replace_text"
            | "replace_in_file"
            | "write_to_file"
            | "create_file"
            | "run_command"
            | "generate_image"
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
        let verdict = if !planned && !auto_approve_command(&settings.permission_mode, command) {
            review(
                settings,
                root,
                judge,
                events,
                &crate::guard::Action::Command(command),
            )
        } else {
            Review::NotAsked
        };
        if matches!(verdict, Review::Unavailable) {
            return Ok(AUTO_UNAVAILABLE.to_owned());
        }
        let (reviewed_ok, note) = verdict.parts();
        if !planned && !reviewed_ok && !auto_approve_command(&settings.permission_mode, command) {
            let details = format!(
                "{}Permission mode: {}\nWorking directory: {}\nShell: {}\n\nExact command to execute:\n{}",
                review_prefix(&note),
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
    if name == "generate_image" {
        return generate_image(settings, root, arguments, events, planned, cancel, actor);
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
        let verdict = if !planned && !auto_approve_file_change(&settings.permission_mode) {
            review(
                settings,
                root,
                judge,
                events,
                &crate::guard::Action::Create {
                    path: &proposal.relative_path,
                    preview: &proposal.content,
                },
            )
        } else {
            Review::NotAsked
        };
        if matches!(verdict, Review::Unavailable) {
            return Ok(AUTO_UNAVAILABLE.to_owned());
        }
        let (reviewed_ok, note) = verdict.parts();
        if !planned && !reviewed_ok && !auto_approve_file_change(&settings.permission_mode) {
            let details = format!(
                "{}Permission mode: {}\n\nProposed complete new-file contents:\n{}",
                review_prefix(&note),
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
        let _ = events.send(PendingEvent::FileChanged {
            path: root.join(&proposal.relative_path),
            name: proposal.relative_path.clone(),
            before: None,
            after: FileContent::Text(proposal.content.clone()),
        });
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
    let verdict = if !planned && !auto_approve_file_change(&settings.permission_mode) {
        review(
            settings,
            root,
            judge,
            events,
            &crate::guard::Action::Edit {
                path: &proposal.relative_path,
                preview: &proposal.preview(),
            },
        )
    } else {
        Review::NotAsked
    };
    if matches!(verdict, Review::Unavailable) {
        return Ok(AUTO_UNAVAILABLE.to_owned());
    }
    let (reviewed_ok, note) = verdict.parts();
    if !planned && !reviewed_ok && !auto_approve_file_change(&settings.permission_mode) {
        let details = format!(
            "{}Permission mode: {}\n\nProposed file change:\n{}",
            review_prefix(&note),
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
    let _ = events.send(PendingEvent::FileChanged {
        path: root.join(&proposal.relative_path),
        name: proposal.relative_path.clone(),
        before: Some(proposal.original.clone()),
        after: FileContent::Text(proposal.updated.clone()),
    });
    Ok(format!(
        "Updated {} ({}).",
        proposal.relative_path, proposal.change_summary
    ))
}

/// Makes an image with the user's image API and saves it as a new file. It costs money, so only
/// Accept Everything (or an approved plan) goes ahead without asking.
fn generate_image(
    settings: &Settings,
    root: &Path,
    arguments: &serde_json::Value,
    events: &mpsc::Sender<PendingEvent>,
    planned: bool,
    cancel: &std::sync::atomic::AtomicBool,
    actor: Option<&str>,
) -> Result<String> {
    let object = arguments
        .as_object()
        .context("tool arguments must be an object")?;
    if object
        .keys()
        .any(|key| !["prompt", "path", "size"].contains(&key.as_str()))
    {
        bail!("generate_image received an unknown argument");
    }
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
    };
    let prompt = text("prompt")
        .filter(|prompt| !prompt.is_empty())
        .context("generate_image requires a `prompt`")?;
    let path = text("path")
        .filter(|path| !path.is_empty())
        .context("generate_image requires a `path`")?;
    if prompt.chars().count() > crate::imagegen::MAX_PROMPT_CHARS {
        bail!(
            "the prompt is longer than {} characters",
            crate::imagegen::MAX_PROMPT_CHARS
        );
    }
    crate::imagegen::check_extension(path)?;
    let size = crate::imagegen::resolve_size(text("size"))?;
    crate::tools::check_new_path(root, path)?;
    let Some(config) = settings
        .image_generation
        .as_ref()
        .filter(|_| crate::imagegen::available(settings))
    else {
        return Ok(
            "Image generation is not set up. The user can add an image API in Settings → General."
                .to_owned(),
        );
    };
    if !planned && settings.permission_mode != "accept-everything" {
        let details = format!(
            "Permission mode: {}\n\nSave to: {path}\nSize: {size}\nImage model: {} at {}\nThis calls your image API and may cost money.\n\nPrompt:\n{prompt}",
            mode_label(&settings.permission_mode),
            config.model,
            config.base_url,
        );
        if !request_tool_approval(
            events,
            titled(actor, format!("Generate image {path}")),
            details,
        )? {
            return Ok(
                "The user declined this image. Do not retry it without new authorization."
                    .to_owned(),
            );
        }
    }
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        bail!("cancelled");
    }
    let key = crate::imagegen::saved_key().context("the image API key is missing")?;
    let bytes = crate::imagegen::generate(config, &key, prompt, &size)?;
    let saved = crate::tools::write_new_bytes(root, path, &bytes)?;
    let _ = events.send(PendingEvent::FileChanged {
        path: root.join(&saved),
        name: saved.clone(),
        before: None,
        after: FileContent::Binary(crate::imagegen::sha256_hex(&bytes)),
    });
    Ok(format!(
        "Saved {saved} ({} KB, {size}) from the image API.",
        (bytes.len() / 1024).max(1)
    ))
}

/// What the model is told when Auto mode cannot check an action because no guard model answered.
const AUTO_UNAVAILABLE: &str = "Auto Mode isn't currently available. Ask the user to switch your mode to Plan, Accept Minimal, Accept Edits, or Manual";

/// The outcome of asking Auto mode's guards about an action.
enum Review {
    /// Not Auto mode (or already decided), so nobody was asked.
    NotAsked,
    /// The guards said it is safe: run it without asking the user.
    Allowed,
    /// The guards said no: ask the user, showing this reason.
    Ask(String),
    /// No guard could answer: do not run it.
    Unavailable,
}

impl Review {
    /// Whether the action may run unasked, and the reason to show when the user is asked.
    fn parts(self) -> (bool, Option<String>) {
        match self {
            Review::Allowed => (true, None),
            Review::Ask(reason) => (false, Some(reason)),
            Review::NotAsked | Review::Unavailable => (false, None),
        }
    }
}

/// In Auto mode, has the guard models look at an action. Other modes never use them.
fn review(
    settings: &Settings,
    root: &Path,
    judge: &dyn crate::guard::Judge,
    events: &mpsc::Sender<PendingEvent>,
    action: &crate::guard::Action<'_>,
) -> Review {
    if settings.permission_mode != "auto" {
        return Review::NotAsked;
    }
    let _ = events.send(PendingEvent::ToolStarted(
        "checking that it is safe".to_owned(),
    ));
    match judge.judge(settings, root, action) {
        crate::guard::Judgement::Allow => Review::Allowed,
        crate::guard::Judgement::Ask(reason) => Review::Ask(reason),
        crate::guard::Judgement::Unavailable => Review::Unavailable,
    }
}

/// Starts the details of an approval that Auto mode's guards sent to the user.
pub(crate) const AUTO_REASON_PREFIX: &str = "Auto mode is asking because: ";

/// The line that tells the user why Auto mode is asking.
fn review_prefix(note: &Option<String>) -> String {
    match note {
        Some(reason) => format!("{AUTO_REASON_PREFIX}{reason}\n\n"),
        None => String::new(),
    }
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
                &crate::guard::NoJudge,
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

#[cfg(test)]
mod compaction_tests {
    use super::*;
    use crate::provider::{ChatMessage, Completion};
    use crate::workflow::Completer;
    use crate::{ModelInfo, ProviderProfile};
    use std::sync::Mutex;

    type Script = Box<dyn Fn(&[ChatMessage]) -> Result<Completion> + Send + Sync>;

    /// A model whose replies the test writes, remembering every request it got.
    struct Model {
        script: Script,
        requests: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl Model {
        fn new(
            script: impl Fn(&[ChatMessage]) -> Result<Completion> + Send + Sync + 'static,
        ) -> Model {
            Model {
                script: Box::new(script),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<Vec<ChatMessage>> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Completer for Model {
        fn complete(
            &self,
            messages: &[ChatMessage],
            _tools: ToolSet,
            _stream: &Stream<'_>,
        ) -> Result<Completion> {
            self.requests.lock().unwrap().push(messages.to_vec());
            (self.script)(messages)
        }
    }

    fn say(text: &str) -> Result<Completion> {
        Ok(Completion {
            text: text.to_owned(),
            provider_id: None,
            model_id: "m".to_owned(),
            failed_over: false,
            tool_calls: Vec::new(),
        })
    }

    fn is_summary_request(messages: &[ChatMessage]) -> bool {
        messages
            .first()
            .is_some_and(|message| message.display.starts_with("You are condensing"))
    }

    fn history() -> Vec<ChatMessage> {
        let big = "z".repeat(10_000);
        let mut messages = vec![ChatMessage::system("sys".to_owned())];
        for turn in 0..4 {
            let question = format!("q{turn} {big}");
            messages.push(ChatMessage::user_with_images(
                question.clone(),
                question,
                Vec::new(),
            ));
            messages.push(ChatMessage::assistant(format!("a{turn} {big}")));
        }
        messages.push(ChatMessage::user_with_images(
            "latest question".to_owned(),
            "latest question".to_owned(),
            Vec::new(),
        ));
        messages
    }

    fn settings_with_window(window: Option<u64>) -> Settings {
        let mut settings = Settings::default();
        let mut profile = ProviderProfile {
            id: "p".to_owned(),
            ..Default::default()
        };
        if let Some(window) = window {
            profile.model_info.insert(
                "m".to_owned(),
                ModelInfo {
                    context: Some(window),
                    ..Default::default()
                },
            );
        }
        settings.providers = vec![profile];
        settings.active_provider_id = Some("p".to_owned());
        settings.model = Some("m".to_owned());
        settings
    }

    /// Runs one turn and returns the answer, and the compactions announced along the way.
    fn run(model: &Model, settings: &Settings) -> (Result<Completion>, Vec<(usize, String)>) {
        let (sender, receiver) = mpsc::channel();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let result = run_loop(
            model,
            settings,
            history(),
            std::path::Path::new("."),
            false,
            &sender,
            &cancel,
        );
        drop(sender);
        let compactions = receiver
            .try_iter()
            .filter_map(|event| match event {
                PendingEvent::Compacted {
                    replaced, summary, ..
                } => Some((replaced, summary)),
                _ => None,
            })
            .collect();
        (result, compactions)
    }

    #[test]
    fn a_conversation_near_the_window_is_condensed_before_the_request_is_sent() {
        let model = Model::new(|messages| {
            if is_summary_request(messages) {
                say("BRIEFING of the earlier work")
            } else {
                say("done")
            }
        });
        let (result, compactions) = run(&model, &settings_with_window(Some(10_000)));
        assert_eq!(result.unwrap().text, "done");
        assert_eq!(compactions.len(), 1, "{compactions:?}");
        assert!(compactions[0].0 > 0);
        let asked = model.asked();
        assert_eq!(asked.len(), 2, "one request to summarize, one real one");
        let real = &asked[1];
        assert!(
            real.iter()
                .any(|message| message.display.contains("BRIEFING")),
            "the model sees the summary"
        );
        assert!(
            real.last().unwrap().display == "latest question",
            "the newest message is untouched"
        );
        assert!(real.len() < history().len(), "the request got shorter");
    }

    #[test]
    fn nothing_is_condensed_while_there_is_plenty_of_room_or_the_window_is_unknown() {
        for window in [Some(1_000_000), None] {
            let model = Model::new(|_| say("done"));
            let (result, compactions) = run(&model, &settings_with_window(window));
            assert_eq!(result.unwrap().text, "done");
            assert!(compactions.is_empty(), "{window:?}");
            assert_eq!(model.asked().len(), 1);
        }
        let mut off = settings_with_window(Some(10_000));
        off.auto_compact = false;
        let model = Model::new(|_| say("done"));
        let (_, compactions) = run(&model, &off);
        assert!(compactions.is_empty(), "switched off");
    }

    #[test]
    fn a_provider_saying_the_request_is_too_large_gets_a_condensed_one() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let model = Model::new(move |messages| {
            if is_summary_request(messages) {
                return say("SHORT BRIEFING");
            }
            if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                anyhow::bail!("This model's maximum context length is 128000 tokens");
            }
            say("answered after condensing")
        });
        // The window is unknown, so nothing was condensed in advance.
        let (result, compactions) = run(&model, &settings_with_window(None));
        assert_eq!(result.unwrap().text, "answered after condensing");
        assert_eq!(compactions.len(), 1);
        let asked = model.asked();
        assert!(
            asked
                .last()
                .unwrap()
                .iter()
                .any(|m| m.display.contains("SHORT BRIEFING"))
        );
    }

    #[test]
    fn other_errors_are_not_treated_as_too_large() {
        let model = Model::new(|_| anyhow::bail!("invalid API key"));
        let (result, compactions) = run(&model, &settings_with_window(None));
        assert!(format!("{:#}", result.unwrap_err()).contains("invalid API key"));
        assert!(compactions.is_empty());
        assert_eq!(model.asked().len(), 1, "no summary was attempted");
    }

    #[test]
    fn a_failed_summary_never_stops_the_turn() {
        let model = Model::new(|messages| {
            if is_summary_request(messages) {
                anyhow::bail!("summarizer is down")
            }
            say("answered anyway")
        });
        let (result, compactions) = run(&model, &settings_with_window(Some(10_000)));
        assert_eq!(result.unwrap().text, "answered anyway");
        assert!(compactions.is_empty());
        let asked = model.asked();
        assert_eq!(
            asked.len(),
            2,
            "tried once, then sent the request as it was"
        );
        assert_eq!(asked[1].len(), history().len());
    }
}

#[cfg(test)]
mod file_change_tests {
    use super::*;

    /// (name, text before, text after)
    type Change = (String, Option<String>, String);

    fn workspace() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("harness-changes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "alpha\nbeta\n").unwrap();
        root.canonicalize().unwrap()
    }

    fn run_tool(
        root: &Path,
        name: &str,
        arguments: serde_json::Value,
    ) -> (Result<String>, Vec<Change>) {
        let mut settings = Settings::default();
        settings.permission_mode = "accept-everything".to_owned();
        let (sender, receiver) = mpsc::channel();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let result = execute_agent_tool(
            &settings,
            root,
            name,
            &arguments,
            &sender,
            &mut Vec::new(),
            &cancel,
            None,
            &crate::guard::NoJudge,
        );
        drop(sender);
        let changes = receiver
            .try_iter()
            .filter_map(|event| match event {
                PendingEvent::FileChanged {
                    name,
                    before,
                    after,
                    ..
                } => Some((
                    name,
                    before,
                    match after {
                        FileContent::Text(text) => text,
                        FileContent::Binary(hash) => format!("binary:{hash}"),
                    },
                )),
                _ => None,
            })
            .collect();
        (result, changes)
    }

    #[test]
    fn an_edit_reports_the_text_before_and_after() {
        let root = workspace();
        let (result, changes) = run_tool(
            &root,
            "replace_text",
            serde_json::json!({"path": "a.txt", "old_text": "beta", "new_text": "gamma"}),
        );
        result.unwrap();
        assert_eq!(
            changes,
            [(
                "a.txt".to_owned(),
                Some("alpha\nbeta\n".to_owned()),
                "alpha\ngamma\n".to_owned()
            )]
        );
    }

    #[test]
    fn a_new_file_reports_no_earlier_text() {
        let root = workspace();
        let (result, changes) = run_tool(
            &root,
            "create_file",
            serde_json::json!({"path": "b.txt", "content": "hello"}),
        );
        result.unwrap();
        assert_eq!(changes, [("b.txt".to_owned(), None, "hello".to_owned())]);
    }

    #[test]
    fn a_change_that_did_not_happen_reports_nothing() {
        let root = workspace();
        let (result, changes) = run_tool(
            &root,
            "replace_text",
            serde_json::json!({"path": "a.txt", "old_text": "not there", "new_text": "x"}),
        );
        assert!(result.is_err());
        assert!(changes.is_empty());
        let (_, reads) = run_tool(&root, "read_file", serde_json::json!({"path": "a.txt"}));
        assert!(reads.is_empty(), "reading is not a change");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "alpha\nbeta\n"
        );
    }
}

#[cfg(test)]
mod auto_guard_tests {
    use super::*;
    use crate::guard::{Action, Judge, Judgement};
    use std::sync::Mutex;

    /// A reviewer with a fixed answer that remembers what it was asked about.
    struct Fixed {
        answer: Option<Option<String>>,
        asked: Mutex<Vec<String>>,
    }

    impl Fixed {
        fn allowing() -> Fixed {
            Fixed {
                answer: None,
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asking(reason: &str) -> Fixed {
            Fixed {
                answer: Some(Some(reason.to_owned())),
                asked: Mutex::new(Vec::new()),
            }
        }

        fn unavailable() -> Fixed {
            Fixed {
                answer: Some(None),
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl Judge for Fixed {
        fn judge(&self, _: &Settings, _: &Path, action: &Action<'_>) -> Judgement {
            let described = match action {
                Action::Command(command) => format!("command: {command}"),
                Action::Edit { path, .. } => format!("edit: {path}"),
                Action::Create { path, .. } => format!("create: {path}"),
            };
            self.asked.lock().unwrap().push(described);
            match &self.answer {
                None => Judgement::Allow,
                Some(Some(reason)) => Judgement::Ask(reason.clone()),
                Some(None) => Judgement::Unavailable,
            }
        }
    }

    fn workspace() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("harness-guard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
        root.canonicalize().unwrap()
    }

    /// Runs one tool call in `mode`. Approval requests are answered with `user_says` and
    /// recorded as (title, details); other events are returned by name.
    fn run(
        mode: &str,
        judge: &dyn Judge,
        root: &Path,
        name: &str,
        arguments: serde_json::Value,
        user_says: bool,
    ) -> (Result<String>, Vec<(String, String)>, Vec<String>) {
        let mut settings = Settings::default();
        settings.permission_mode = mode.to_owned();
        let (sender, receiver) = mpsc::channel();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let listener = scope.spawn(move || {
                let mut approvals = Vec::new();
                let mut others = Vec::new();
                while let Ok(event) = receiver.recv() {
                    match event {
                        PendingEvent::ApprovalRequest(request) => {
                            approvals.push((request.title.clone(), request.details.clone()));
                            let _ = request.response.send(user_says);
                        }
                        PendingEvent::ToolStarted(label) => others.push(label),
                        _ => {}
                    }
                }
                (approvals, others)
            });
            let result = execute_agent_tool(
                &settings,
                root,
                name,
                &arguments,
                &sender,
                &mut Vec::new(),
                &cancel,
                None,
                judge,
            );
            drop(sender);
            let (approvals, others) = listener.join().unwrap();
            (result, approvals, others)
        })
    }

    #[test]
    fn a_command_the_reviewer_allows_runs_without_asking_the_user() {
        let root = workspace();
        let judge = Fixed::allowing();
        let (result, approvals, started) = run(
            "auto",
            &judge,
            &root,
            "run_command",
            serde_json::json!({"command": "echo guard-ran"}),
            false,
        );
        assert!(result.unwrap().contains("guard-ran"));
        assert!(approvals.is_empty(), "{approvals:?}");
        assert_eq!(judge.asked(), ["command: echo guard-ran"]);
        assert!(
            started
                .iter()
                .any(|label| label.contains("checking that it is safe")),
            "{started:?}"
        );
    }

    #[test]
    fn a_command_the_reviewer_doubts_goes_to_the_user_with_the_reason() {
        let root = workspace();
        let judge = Fixed::asking("it reaches outside the project");
        let (result, approvals, _) = run(
            "auto",
            &judge,
            &root,
            "run_command",
            serde_json::json!({"command": "echo not-run"}),
            false,
        );
        assert!(result.unwrap().contains("declined"), "the user said no");
        assert_eq!(approvals.len(), 1);
        assert!(
            approvals[0]
                .1
                .starts_with("Auto mode is asking because: it reaches outside the project"),
            "{}",
            approvals[0].1
        );
        assert!(
            approvals[0].1.contains("echo not-run"),
            "the command is still shown"
        );
        // If the user approves, it runs.
        let (result, approvals, _) = run(
            "auto",
            &Fixed::asking("unsure"),
            &root,
            "run_command",
            serde_json::json!({"command": "echo user-said-yes"}),
            true,
        );
        assert!(result.unwrap().contains("user-said-yes"));
        assert_eq!(approvals.len(), 1);
    }

    #[test]
    fn the_other_modes_never_consult_the_reviewer() {
        let root = workspace();
        for mode in ["accept-edits", "accept-minimal"] {
            let judge = Fixed::allowing();
            let (result, approvals, started) = run(
                mode,
                &judge,
                &root,
                "run_command",
                serde_json::json!({"command": "echo plain"}),
                true,
            );
            assert!(result.unwrap().contains("plain"), "{mode}");
            assert_eq!(
                approvals.len(),
                1,
                "{mode}: these modes ask the user as before"
            );
            assert!(judge.asked().is_empty(), "{mode}");
            assert!(started.is_empty(), "{mode}: {started:?}");
        }
        let judge = Fixed::allowing();
        let (_, approvals, _) = run(
            "accept-everything",
            &judge,
            &root,
            "run_command",
            serde_json::json!({"command": "echo all"}),
            false,
        );
        assert!(approvals.is_empty() && judge.asked().is_empty());
    }

    #[test]
    fn in_auto_mode_every_edit_and_new_file_gets_the_guards_verdict() {
        let root = workspace();
        let judge = Fixed::allowing();
        let (result, approvals, _) = run(
            "auto",
            &judge,
            &root,
            "replace_text",
            serde_json::json!({"path": "a.txt", "old_text": "alpha", "new_text": "beta"}),
            false,
        );
        result.unwrap();
        assert!(approvals.is_empty());
        assert_eq!(
            judge.asked(),
            ["edit: a.txt"],
            "even a small edit is checked"
        );
        let (result, approvals, _) = run(
            "auto",
            &judge,
            &root,
            "create_file",
            serde_json::json!({"path": "new.txt", "content": "hi"}),
            false,
        );
        result.unwrap();
        assert!(approvals.is_empty());
        assert_eq!(judge.asked(), ["edit: a.txt", "create: new.txt"]);
        assert!(root.join("new.txt").exists());
    }

    #[test]
    fn manual_mode_asks_the_user_every_time_and_never_the_guards() {
        let root = workspace();
        let judge = Fixed::allowing();
        let (result, approvals, started) = run(
            "manual",
            &judge,
            &root,
            "run_command",
            serde_json::json!({"command": "cargo test"}),
            true,
        );
        result.unwrap();
        assert_eq!(approvals.len(), 1);
        let (_, approvals, _) = run(
            "manual",
            &judge,
            &root,
            "replace_text",
            serde_json::json!({"path": "a.txt", "old_text": "alpha", "new_text": "beta"}),
            true,
        );
        assert_eq!(approvals.len(), 1);
        assert!(judge.asked().is_empty() && started.is_empty());
    }

    #[test]
    fn when_no_guard_can_answer_nothing_runs_and_the_model_is_told_why() {
        let root = workspace();
        let judge = Fixed::unavailable();
        let want = "Auto Mode isn't currently available. Ask the user to switch your mode to Plan, Accept Minimal, Accept Edits, or Manual";
        for (name, arguments) in [
            ("run_command", serde_json::json!({"command": "echo never"})),
            (
                "replace_text",
                serde_json::json!({"path": "a.txt", "old_text": "alpha", "new_text": "beta"}),
            ),
            (
                "create_file",
                serde_json::json!({"path": "never.txt", "content": "x"}),
            ),
        ] {
            let (result, approvals, _) = run("auto", &judge, &root, name, arguments, true);
            assert_eq!(result.unwrap(), want, "{name}");
            assert!(approvals.is_empty(), "{name}: the user is not asked either");
        }
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "alpha\n"
        );
        assert!(!root.join("never.txt").exists());
    }

    #[test]
    fn an_edit_the_reviewer_doubts_waits_for_the_user_and_is_not_applied_if_declined() {
        let root = workspace();
        let big = "y".repeat(3_000);
        let judge = Fixed::asking("it rewrites a lot");
        let (result, approvals, _) = run(
            "auto",
            &judge,
            &root,
            "create_file",
            serde_json::json!({"path": "doubted.txt", "content": big}),
            false,
        );
        assert!(result.unwrap().contains("declined"));
        assert!(!root.join("doubted.txt").exists(), "nothing was written");
        assert!(approvals[0].1.contains("it rewrites a lot"));
    }

    #[test]
    fn plan_mode_still_needs_an_approved_plan_whatever_the_reviewer_says() {
        let root = workspace();
        let judge = Fixed::allowing();
        let (result, approvals, _) = run(
            "plan",
            &judge,
            &root,
            "run_command",
            serde_json::json!({"command": "echo sneaky"}),
            true,
        );
        assert!(result.unwrap().contains("Plan mode blocks"));
        assert!(approvals.is_empty() && judge.asked().is_empty());
    }
}

#[cfg(test)]
mod image_tool_tests {
    use super::*;
    use crate::guard::NoJudge;
    use crate::imagegen::{ImageConfig, key_name};
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nthe-bytes-of-a-tiny-placeholder";

    /// What running the tool gave: its answer, the approvals asked, and the files reported.
    type Outcome = (
        Result<String>,
        Vec<(String, String)>,
        Vec<(std::path::PathBuf, FileContent)>,
    );

    fn workspace() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("harness-image-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    /// A server that answers with the test PNG, and settings pointing at it with a saved key.
    fn configured(mode: &str, answers: usize) -> (Settings, crate::testutil::Requests) {
        let body: &'static str = Box::leak(
            serde_json::json!({"data": [{"b64_json": BASE64.encode(PNG)}]})
                .to_string()
                .into_boxed_str(),
        );
        let (base, seen) =
            crate::testutil::serve_full(vec![(200, "application/json", body); answers]);
        let mut settings = Settings::default();
        settings.permission_mode = mode.to_owned();
        settings.image_generation = Some(ImageConfig {
            base_url: base,
            model: "test-image-model".to_owned(),
        });
        crate::secrets::store(&key_name(), "image-key").unwrap();
        (settings, seen)
    }

    /// Runs the tool; approval requests are answered with `user_says` and returned.
    fn run(
        settings: &Settings,
        root: &Path,
        arguments: serde_json::Value,
        user_says: bool,
        approved_plan: &mut Vec<(String, serde_json::Value)>,
    ) -> Outcome {
        let (sender, receiver) = mpsc::channel();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let listener = scope.spawn(move || {
                let mut approvals = Vec::new();
                let mut changes = Vec::new();
                while let Ok(event) = receiver.recv() {
                    match event {
                        PendingEvent::ApprovalRequest(request) => {
                            approvals.push((request.title.clone(), request.details.clone()));
                            let _ = request.response.send(user_says);
                        }
                        PendingEvent::FileChanged { path, after, .. } => {
                            changes.push((path, after))
                        }
                        _ => {}
                    }
                }
                (approvals, changes)
            });
            let result = execute_agent_tool(
                settings,
                root,
                "generate_image",
                &arguments,
                &sender,
                approved_plan,
                &cancel,
                None,
                &NoJudge,
            );
            drop(sender);
            let (approvals, changes) = listener.join().unwrap();
            (result, approvals, changes)
        })
    }

    fn args(path: &str) -> serde_json::Value {
        serde_json::json!({"prompt": "a calm blue gradient", "path": path, "size": "landscape"})
    }

    #[test]
    fn in_accept_everything_the_image_is_made_and_saved_without_asking() {
        let root = workspace();
        let (settings, seen) = configured("accept-everything", 1);
        let (result, approvals, changes) = run(
            &settings,
            &root,
            args("assets/hero.png"),
            false,
            &mut Vec::new(),
        );
        let message = result.unwrap();
        assert!(
            message.contains("Saved assets/hero.png") && message.contains("1536x1024"),
            "{message}"
        );
        assert!(approvals.is_empty());
        assert_eq!(
            std::fs::read(root.join("assets").join("hero.png")).unwrap(),
            PNG,
            "folders were created"
        );
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let sent: serde_json::Value = serde_json::from_str(&requests[0].1).unwrap();
        assert_eq!(sent["prompt"], "a calm blue gradient");
        assert_eq!(sent["size"], "1536x1024");
        assert_eq!(sent["model"], "test-image-model");
        assert_eq!(changes.len(), 1, "undo is told");
        assert_eq!(
            changes[0].1,
            FileContent::Binary(crate::imagegen::sha256_hex(PNG))
        );
        let _ = crate::secrets::delete(&key_name());
    }

    #[test]
    fn every_other_mode_asks_first_because_it_costs_money_and_a_no_calls_nothing() {
        for mode in ["accept-edits", "accept-minimal", "auto"] {
            let root = workspace();
            let (settings, seen) = configured(mode, 1);
            let (result, approvals, changes) =
                run(&settings, &root, args("a.png"), false, &mut Vec::new());
            assert!(result.unwrap().contains("declined"), "{mode}");
            assert_eq!(approvals.len(), 1, "{mode}");
            assert!(
                approvals[0].0.contains("Generate image a.png"),
                "{}",
                approvals[0].0
            );
            assert!(
                approvals[0].1.contains("may cost money")
                    && approvals[0].1.contains("a calm blue gradient"),
                "{}",
                approvals[0].1
            );
            assert!(
                approvals[0].1.contains("test-image-model"),
                "{}",
                approvals[0].1
            );
            assert!(
                seen.lock().unwrap().is_empty(),
                "{mode}: the API was not called"
            );
            assert!(!root.join("a.png").exists() && changes.is_empty(), "{mode}");
        }
        let root = workspace();
        let (settings, seen) = configured("accept-edits", 1);
        let (result, approvals, _) = run(&settings, &root, args("yes.png"), true, &mut Vec::new());
        assert!(result.unwrap().contains("Saved yes.png"));
        assert_eq!(approvals.len(), 1);
        assert_eq!(seen.lock().unwrap().len(), 1);
        let _ = crate::secrets::delete(&key_name());
    }

    #[test]
    fn nothing_is_overwritten_and_nothing_is_called_for_a_bad_request() {
        let root = workspace();
        std::fs::write(root.join("taken.png"), "mine").unwrap();
        let (settings, seen) = configured("accept-everything", 4);
        let cases = [
            (args("taken.png"), "already exists"),
            (args("notes.txt"), "must end in .png"),
            (args("../escape.png"), ""),
            (args("/etc/escape.png"), ""),
            (
                serde_json::json!({"prompt": "x", "path": "a.png", "size": "huge"}),
                "size must be",
            ),
            (
                serde_json::json!({"prompt": "  ", "path": "a.png"}),
                "requires a `prompt`",
            ),
            (serde_json::json!({"prompt": "x"}), "requires a `path`"),
            (
                serde_json::json!({"prompt": "x", "path": "a.png", "extra": 1}),
                "unknown argument",
            ),
        ];
        for (arguments, expected) in cases {
            let (result, approvals, changes) =
                run(&settings, &root, arguments.clone(), true, &mut Vec::new());
            let error = format!("{:#}", result.expect_err(&arguments.to_string()));
            assert!(error.contains(expected), "{arguments}: {error}");
            assert!(approvals.is_empty() && changes.is_empty(), "{arguments}");
        }
        assert_eq!(
            std::fs::read_to_string(root.join("taken.png")).unwrap(),
            "mine"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "no request was made for any of them"
        );
        let _ = crate::secrets::delete(&key_name());
    }

    #[test]
    fn without_a_setup_the_tool_says_so_and_calls_nothing() {
        let root = workspace();
        let mut settings = Settings::default();
        settings.permission_mode = "accept-everything".to_owned();
        let (result, approvals, _) = run(&settings, &root, args("a.png"), true, &mut Vec::new());
        assert!(result.unwrap().contains("not set up"));
        assert!(approvals.is_empty() && !root.join("a.png").exists());
    }

    #[test]
    fn plan_mode_needs_the_image_in_the_approved_plan() {
        let root = workspace();
        let (settings, seen) = configured("plan", 1);
        let (result, approvals, _) = run(&settings, &root, args("p.png"), true, &mut Vec::new());
        assert!(result.unwrap().contains("Plan mode blocks"));
        assert!(approvals.is_empty() && seen.lock().unwrap().is_empty());
        // Once the plan approved exactly this call it goes ahead, and then without asking again.
        let mut plan = vec![("generate_image".to_owned(), args("p.png"))];
        let (result, approvals, _) = run(&settings, &root, args("p.png"), false, &mut plan);
        assert!(result.unwrap().contains("Saved p.png"));
        assert!(approvals.is_empty() && plan.is_empty());
        let _ = crate::secrets::delete(&key_name());
    }

    #[test]
    fn only_the_main_assistant_is_offered_the_tool_and_only_when_asked_for() {
        use crate::tools::ToolSet;
        let main = |images| ToolSet::Main {
            plan_mode: false,
            workflows: false,
            images,
        };
        assert!(main(true).allows("generate_image"));
        assert!(!main(false).allows("generate_image"));
        assert!(!ToolSet::Explore.allows("generate_image"));
        assert!(
            !ToolSet::Implement.allows("generate_image"),
            "subagents cannot spend money"
        );
        assert!(!ToolSet::None.allows("generate_image"));
    }
}
