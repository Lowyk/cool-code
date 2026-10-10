use crate::agent::{PendingEvent, run_agent_turns};
use crate::policy::MODES;
use crate::tui::context::InstructionFiles;
use crate::tui::effort::effort_name;
use crate::tui::models::selected_model_name;
use crate::tui::pickers::model::ModelPicker;
use crate::tui::settings::Section;
use crate::tui::settings::auto_mode::ACTIVATE_NOTICE;
use crate::tui::state::{
    App, LEVELS, PrivacyPrompt, StreamingTurn, TranscriptEntry, TranscriptKind, mode_alias,
};
use crate::{Effort, provider, write_settings};
use anyhow::{Context, Result};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

pub(super) fn format_provider_error(error: &str) -> (String, String) {
    let status = error
        .split_once("provider returned ")
        .and_then(|(_, rest)| rest.split_once(": ").map(|(status, _)| status.trim()))
        .filter(|status| !status.is_empty())
        .unwrap_or("Request failed")
        .to_owned();
    let body = error.find('{').map(|start| &error[start..]);
    let message = body
        .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    value
                        .get("error")
                        .filter(|error| !error.is_null())
                        .map(|error| error.to_string())
                })
        })
        .or_else(|| {
            error
                .split_once("provider returned ")
                .and_then(|(_, rest)| rest.split_once(": ").map(|(_, body)| body.trim()))
                .filter(|body| !body.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| error.to_owned());
    let message = if status.starts_with("401") || status.starts_with("403") {
        format!(
            "{message}\n\nThe provider rejected the API key. Open /provider, select this provider, press e, type a new API key, and save."
        )
    } else {
        message
    };
    (status, message)
}

/// The effort name as typed, lower-cased, with the former name of the top tier mapped over.
fn canonical_effort_name(requested: &str) -> String {
    let name = requested.trim().to_ascii_lowercase();
    if name == "extreme" {
        "ultimate".to_owned()
    } else {
        name
    }
}

impl App {
    pub(super) fn submit(&mut self) -> Result<()> {
        let value = self.input.trim().to_owned();
        if value.is_empty() {
            return Ok(());
        }
        if self.pending.is_some() && !value.starts_with('/') {
            self.notice = "Waiting for the current model response.".to_owned();
            return Ok(());
        }
        self.input.clear();

        if value.starts_with('/') {
            self.transcript.push(TranscriptEntry {
                kind: TranscriptKind::User,
                text: value.clone(),
            });
        }

        if value == "/effort" {
            self.open_effort_picker();
            self.finish_command(if self.picker {
                "Opened the effort selector.".to_owned()
            } else {
                self.notice.clone()
            });
            return Ok(());
        }
        if value == "/settings" {
            self.open_settings(Section::General);
            self.finish_command("Opened Settings.");
            return Ok(());
        }
        if value == "/usage" {
            self.open_usage();
            self.finish_command("Opened provider usage.");
            return Ok(());
        }
        if value == "/stats" {
            self.open_stats(false);
            self.finish_command("Opened usage stats.");
            return Ok(());
        }
        if value == "/stats clear" {
            self.open_stats(true);
            self.finish_command("Confirm deleting the usage history.");
            return Ok(());
        }
        if value == "/provider" {
            self.open_settings(Section::Providers);
            self.finish_command("Opened providers in Settings.");
            return Ok(());
        }
        if value == "/chain" {
            self.open_settings(Section::AutoSwitch);
            self.finish_command("Opened model chains in Settings.");
            return Ok(());
        }
        if let Some(chain_id) = value.strip_prefix("/chain ") {
            self.activate_chain(chain_id.trim())?;
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/mode" {
            self.mode_picker = true;
            self.finish_command("Opened the permission mode selector.");
            return Ok(());
        }
        if let Some(requested) = value.strip_prefix("/mode ") {
            if let Some((_, mode)) = MODES.iter().find(|(label, mode)| {
                *mode == requested.trim().to_ascii_lowercase()
                    || label.eq_ignore_ascii_case(requested.trim())
                    || mode_alias(mode) == requested.trim().to_ascii_lowercase()
            }) {
                if *mode == "auto" && !self.settings.auto_ready() {
                    self.notice = ACTIVATE_NOTICE.to_owned();
                } else {
                    self.settings.permission_mode = (*mode).to_owned();
                    self.mode_index = crate::policy::mode_index(mode);
                    write_settings(&self.settings)?;
                    self.notice = format!("Mode set to {}.", requested.trim());
                }
            } else {
                self.notice = "Choose auto, edits, minimal, all, manual, or plan.".to_owned();
            }
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/init" {
            let path = std::env::current_dir()?.join("COOL.md");
            let template = "# COOL.md\n\nProject guidance for Cool Code.\n\n## Conventions\n- Add project-specific coding conventions here.\n\n## Commands\n- Add build, test, and lint commands here.\n\n## Notes\n- Keep this file concise; it is included as project context for the model.\n";
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write as _;
                    file.write_all(template.as_bytes())?;
                    self.notice = format!("Created {}.", path.display());
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.notice = format!("{} already exists; left it unchanged.", path.display());
                }
                Err(error) => return Err(error).context("creating COOL.md"),
            }
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if let Some(requested) = value.strip_prefix("/effort ") {
            if let Some(index) = LEVELS
                .iter()
                .position(|level| effort_name(*level) == canonical_effort_name(requested))
            {
                self.picker_index = index;
                let selected = LEVELS[index];
                if self.workflow_tier_is_locked(selected) {
                    self.picker = false;
                } else if selected == Effort::Ultimate && !self.settings.ultimate_acknowledged {
                    self.picker = false;
                    self.confirm_ultimate = true;
                } else {
                    self.apply_effort(selected)?;
                }
            } else {
                self.notice =
                    "Choose low, medium, high, xhigh, max, super, or ultimate.".to_owned();
            }
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/model" {
            self.model_picker = Some(ModelPicker::new(&self.settings));
            self.finish_command("Opened the model picker.");
            return Ok(());
        }
        if value == "/forcemodel" || value.starts_with("/forcemodel ") {
            let model = value.trim_start_matches("/forcemodel").trim();
            if model.is_empty() {
                self.notice = "Usage: /forcemodel <model-id> (sets it on the default provider, whatever the automatic-switching rules say)".to_owned();
            } else {
                self.force_model(model)?;
            }
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if let Some(model) = value.strip_prefix("/model ") {
            let model = model.trim();
            if model.is_empty() {
                self.notice = "Usage: /model <model-id>".to_owned();
            } else {
                self.select_model(model)?;
            }
            self.finish_command(if self.model_choices.is_some() {
                "Choose which provider should supply that model.".to_owned()
            } else {
                self.notice.clone()
            });
            return Ok(());
        }
        for (command, claude) in [("/claudemd", true), ("/agentsmd", false)] {
            if value == command || value.starts_with(&format!("{command} ")) {
                let argument = value[command.len()..].trim().to_ascii_lowercase();
                self.toggle_project_instructions(claude, &argument)?;
                self.finish_command(self.notice.clone());
                return Ok(());
            }
        }
        if value == "/resume" || value == "/resume all" {
            self.open_session_picker(value == "/resume all");
            let notice = if self.session_picker.is_some() {
                "Choose a session to continue.".to_owned()
            } else {
                self.notice.clone()
            };
            self.finish_command(notice);
            return Ok(());
        }
        if value == "/quit" || value == "/exit" {
            self.save_session();
            self.finish_command("Goodbye.");
            self.running = false;
            return Ok(());
        }
        if value == "/help" {
            self.notice = "Commands: /help, /settings, /usage, /stats, /model <id|author/id>, /forcemodel <id>, /compact, /undo, /mode [name], /chain [id], /effort [level], /files, /read <path>, /search <text>, /git status, /init, /privacy [add|clear|revoke], /claudemd, /agentsmd, /resume [all], /clear, /quit. Attach workspace files with @path.".to_owned();
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/files"
            || value == "/git status"
            || value.starts_with("/read ")
            || value.starts_with("/search ")
        {
            let output = self.run_readonly_tool(&value);
            self.notice = if output.starts_with("Tool error:") {
                output.clone()
            } else {
                "Read-only workspace tool completed.".to_owned()
            };
            self.finish_command(output);
            return Ok(());
        }
        if let Some(value) = value.strip_prefix("/privacy add ") {
            let value = value.trim();
            if value.chars().count() < 3 {
                self.notice = "Privacy values must be at least 3 characters.".to_owned();
                self.finish_command(self.notice.clone());
                return Ok(());
            }
            let mut values = provider::load_redaction_values()?;
            if !values.iter().any(|existing| existing == value) {
                values.push(value.to_owned());
                provider::save_redaction_values(&values)?;
            }
            self.notice = "Value saved to the OS credential store; it will be redacted locally for privacy-sensitive models.".to_owned();
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/privacy clear" {
            provider::save_redaction_values(&[])?;
            self.notice =
                "Custom local redaction values cleared from the OS credential store.".to_owned();
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/privacy revoke" {
            self.settings.privacy_acknowledged.clear();
            self.settings.privacy_image_acknowledged.clear();
            write_settings(&self.settings)?;
            self.notice =
                "Privacy and image-content acknowledgements cleared; risky models will ask again before sending."
                    .to_owned();
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/privacy" {
            let count = provider::load_redaction_values()?.len();
            self.notice = format!(
                "{} custom local redaction value(s). Use /privacy add <value>, /privacy clear, or /privacy revoke.",
                count
            );
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        if value == "/undo" {
            let message = self.undo_last();
            self.finish_command(message.clone());
            self.notice = message;
            return Ok(());
        }
        if value == "/compact" {
            let message = self.start_compaction()?;
            self.finish_command(message);
            return Ok(());
        }
        if value == "/clear" {
            if self.pending.is_some() {
                self.finish_command(
                    "Wait for the current turn to finish before clearing the conversation.",
                );
                return Ok(());
            }
            self.save_session();
            self.messages.clear();
            self.transcript.clear();
            self.begin_new_session();
            self.history_scroll = 0;
            self.notice = "Conversation cleared.".to_owned();
            return Ok(());
        }
        if value.starts_with('/') {
            self.notice =
                "Try /help, /settings, /model, /mode, /effort, /init, /clear, or /quit.".to_owned();
            self.finish_command(self.notice.clone());
            return Ok(());
        }
        self.send_prompt(value, &std::collections::HashSet::new())
    }

    /// Sends a message whose `@` references have been read, after the privacy checks.
    pub(super) fn send_built_message(&mut self, user_message: provider::ChatMessage) -> Result<()> {
        if let Some(risk) = provider::privacy_risk_for_settings(&self.settings) {
            let needs_warning = !self
                .settings
                .privacy_acknowledged
                .iter()
                .any(|ack| ack == risk);
            let needs_image_choice = provider::message_contains_image(&user_message)
                && !self
                    .settings
                    .privacy_image_acknowledged
                    .iter()
                    .any(|ack| ack == risk);
            if needs_warning || needs_image_choice {
                self.pending_privacy_message = Some(user_message);
                self.privacy_confirmation = Some(PrivacyPrompt {
                    risk: risk.to_owned(),
                    allow_images: self
                        .settings
                        .privacy_image_acknowledged
                        .iter()
                        .any(|ack| ack == risk),
                });
                return Ok(());
            }
        }
        self.dispatch_user_message(user_message)
    }

    /// Condenses the older conversation in the background; returns what to tell the user.
    pub(super) fn start_compaction(&mut self) -> Result<String> {
        if self.pending.is_some() {
            return Ok(
                "Wait for the current turn to finish before condensing the conversation."
                    .to_owned(),
            );
        }
        let system_prompt = self.build_system_prompt()?;
        let mut request_messages = vec![provider::ChatMessage::system(system_prompt)];
        request_messages.extend(self.messages.iter().cloned());
        if crate::context::split_point(&request_messages).is_none() {
            return Ok("There is not enough conversation to condense yet.".to_owned());
        }
        let settings = self.settings.clone();
        let (sender, receiver) = mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.streaming = Some(StreamingTurn::new(cancel.clone()));
        thread::spawn(move || {
            let ignore = |_event| {};
            let stream = crate::stream::Stream {
                on_event: &ignore,
                cancel: &cancel,
            };
            let completer = crate::workflow::ProviderCompleter(&settings);
            let finished = match crate::context::compact(&completer, &request_messages, &stream) {
                Ok(compaction) => {
                    crate::agent::announce_compaction(&sender, &compaction);
                    Ok(())
                }
                Err(error) => Err(format!("{error:#}")),
            };
            let _ = sender.send(PendingEvent::CompactFinished(finished));
        });
        self.pending = Some(receiver);
        self.notice = "Condensing the conversation…".to_owned();
        Ok("Condensing the earlier conversation into a summary…".to_owned())
    }

    pub(super) fn dispatch_user_message(
        &mut self,
        user_message: provider::ChatMessage,
    ) -> Result<()> {
        let user_message = match self.pending_note.take() {
            Some(note) => crate::tui::undo::with_note(user_message, &note),
            None => user_message,
        };
        self.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: user_message.display.clone(),
        });
        self.messages.push(user_message);
        self.save_session();
        self.history_scroll = 0;
        let system_prompt = self.build_system_prompt()?;
        let mut request_messages = vec![provider::ChatMessage::system(system_prompt)];
        request_messages.extend(self.messages.iter().cloned());
        let settings = self.settings.clone();
        let workspace_root = std::env::current_dir()?
            .canonicalize()
            .context("resolving workspace root")?;
        let workspace_trusted = self.workspace_trusted;
        let (sender, receiver) = mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.streaming = Some(StreamingTurn::new(cancel.clone()));
        self.tracker = crate::tui::tracker::Tracker::default();
        thread::spawn(move || {
            let result = run_agent_turns(
                settings,
                request_messages,
                workspace_root,
                workspace_trusted,
                &sender,
                cancel,
            )
            .map_err(|error| format!("{error:#}"));
            let _ = sender.send(PendingEvent::Finished(result));
        });
        self.pending = Some(receiver);
        self.notice = "Thinking…".to_owned();
        Ok(())
    }

    /// Everything the model is told before the conversation: the built-in policy, the active
    /// permission mode, and the instruction files the user opted into.
    pub(super) fn build_system_prompt(&mut self) -> Result<String> {
        let root = std::env::current_dir()?
            .canonicalize()
            .context("resolving workspace root")?;
        let (system_prompt, warnings) = crate::prompt::assemble(
            &self.settings,
            self.workspace_trusted,
            &root,
            &self.projects_path,
        )?;
        if !warnings.is_empty() {
            self.notice = format!("Skipped instruction files: {}", warnings.join("; "));
        }
        Ok(system_prompt)
    }

    /// The optional instruction files in effect for `root`: the global defaults, overridden by
    /// anything this project chose with /claudemd or /agentsmd.
    fn instruction_files(&self, root: &std::path::Path) -> InstructionFiles {
        let registry = crate::projects::Registry::load_from(&self.projects_path);
        InstructionFiles {
            project_claude: registry.loads_claude_md(root, self.settings.default_load_claude_md),
            project_agents: registry.loads_agents_md(root, self.settings.default_load_agents_md),
            global_claude: self.settings.load_global_claude_md,
        }
    }

    /// `/claudemd` and `/agentsmd`: switch loading of that file for the current project.
    fn toggle_project_instructions(&mut self, claude: bool, argument: &str) -> Result<()> {
        let name = if claude { "CLAUDE.md" } else { "AGENTS.md" };
        let command = if claude { "/claudemd" } else { "/agentsmd" };
        let root = std::env::current_dir()?
            .canonicalize()
            .context("resolving workspace root")?;
        let current = {
            let files = self.instruction_files(&root);
            if claude {
                files.project_claude
            } else {
                files.project_agents
            }
        };
        let next = match argument {
            "" => !current,
            "on" => true,
            "off" => false,
            _ => {
                self.notice = format!("Usage: {command} [on|off]");
                return Ok(());
            }
        };
        crate::projects::update(&self.projects_path, |registry| {
            if claude {
                registry.set_claude_md(&root, next);
            } else {
                registry.set_agents_md(&root, next);
            }
        })?;
        let mut notice = format!(
            "{name} loading is {} for this project.",
            if next { "ON" } else { "OFF" }
        );
        if next && !root.join(name).is_file() {
            notice.push_str(&format!(" (There is no {name} in this folder yet.)"));
        }
        if next && !self.workspace_trusted {
            notice.push_str(" It takes effect once the folder is trusted.");
        }
        self.notice = notice;
        Ok(())
    }

    /// Stops the running turn immediately; the worker notices the flag and exits on its own.
    pub(super) fn cancel_turn(&mut self) {
        // Apply whatever the worker already sent: the turn may have just finished, and tool-call
        // messages still queued must reach the context before it is repaired below.
        self.poll_response();
        let Some(turn) = self.streaming.take() else {
            return;
        };
        turn.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.pending = None;
        self.commit_checkpoint();
        self.answer_unfinished_tool_calls();
        self.keep_partial_answer(&turn.text, true);
        self.notice = "Turn cancelled.".to_owned();
    }

    /// Gives every tool call without a result a cancelled result, so the next request stays valid.
    fn answer_unfinished_tool_calls(&mut self) {
        let answered = self
            .messages
            .iter()
            .filter_map(|message| message.tool_call_id.clone())
            .collect::<Vec<_>>();
        let unanswered = self
            .messages
            .iter()
            .filter_map(|message| message.tool_calls.as_ref())
            .flatten()
            .filter_map(|call| {
                let id = call.get("id")?.as_str()?;
                let name = call.pointer("/function/name")?.as_str()?;
                (!answered.iter().any(|answered| answered == id))
                    .then(|| (id.to_owned(), name.to_owned()))
            })
            .collect::<Vec<_>>();
        for (id, name) in unanswered {
            self.messages.push(provider::ChatMessage::tool_result(
                id,
                name,
                "Cancelled by the user before this tool ran.".to_owned(),
            ));
        }
    }

    /// Keeps an interrupted answer visible and in context so the model can continue from it.
    fn keep_partial_answer(&mut self, partial: &str, user_cancelled: bool) {
        if !partial.trim().is_empty() {
            self.transcript.push(TranscriptEntry {
                kind: TranscriptKind::Assistant,
                text: partial.to_owned(),
            });
            self.messages.push(provider::ChatMessage::assistant(format!(
                "{partial}\n\n[interrupted by the user]"
            )));
        }
        // A failure with nothing to keep is just an error, not an interruption.
        if user_cancelled || !partial.trim().is_empty() {
            self.transcript.push(TranscriptEntry {
                kind: TranscriptKind::CommandOutput,
                text: "(interrupted)".to_owned(),
            });
        }
        self.history_scroll = 0;
    }

    /// Applies every event the worker has sent since the last frame.
    pub(super) fn poll_response(&mut self) {
        self.poll_tasks();
        if let Some(turn) = self.streaming.as_mut() {
            turn.prune_arrivals(std::time::Instant::now());
        }
        let was_pending = self.pending.is_some();
        while let Some(receiver) = self.pending.as_ref() {
            let event = receiver.try_recv();
            let keep_going = matches!(
                event,
                Ok(PendingEvent::TextDelta(_)
                    | PendingEvent::Usage(_)
                    | PendingEvent::ToolStarted(_)
                    | PendingEvent::ToolAction(_)
                    | PendingEvent::Compacted { .. }
                    | PendingEvent::FileChanged { .. }
                    | PendingEvent::Subagent(_)
                    | PendingEvent::ConversationMessage(_))
            );
            self.apply_pending_event(event);
            if !keep_going {
                break;
            }
        }
        if was_pending && self.pending.is_none() {
            self.save_session();
            self.check_usage_after_turn();
        }
    }

    fn apply_pending_event(&mut self, event: Result<PendingEvent, TryRecvError>) {
        let now = std::time::Instant::now();
        match event {
            Ok(PendingEvent::TextDelta(delta)) => {
                if let Some(turn) = self.streaming.as_mut() {
                    turn.arrivals.push((turn.text.len(), now));
                    turn.text.push_str(&delta);
                    turn.tool = None;
                }
            }
            Ok(PendingEvent::Usage(tokens)) => {
                if let Some(turn) = self.streaming.as_mut() {
                    turn.usage = Some(tokens);
                }
            }
            Ok(PendingEvent::ToolStarted(label)) => {
                if let Some(turn) = self.streaming.as_mut() {
                    turn.tool = Some((label, now));
                }
            }
            Ok(PendingEvent::ToolAction(action)) => {
                if let Some(turn) = self.streaming.as_mut() {
                    turn.tool = None;
                }
                self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::CommandOutput,
                    text: action,
                });
                self.notice = "Workspace tool completed; continuing model turn…".to_owned();
            }
            Ok(PendingEvent::ConversationMessage(message)) => {
                // Text streamed before a tool round becomes its own transcript entry.
                if message.role == "assistant"
                    && message.tool_calls.is_some()
                    && let Some(turn) = self.streaming.as_mut()
                    && !turn.text.trim().is_empty()
                {
                    self.transcript.push(TranscriptEntry {
                        kind: TranscriptKind::Assistant,
                        text: std::mem::take(&mut turn.text),
                    });
                    turn.arrivals.clear();
                }
                self.messages.push(message);
            }
            Ok(PendingEvent::Compacted {
                replaced,
                with,
                summary,
            }) => {
                // The conversation the worker sees and this one stay the same list.
                if replaced <= self.messages.len() {
                    self.messages.splice(0..replaced, with);
                }
                self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::CommandOutput,
                    text: summary,
                });
                self.history_scroll = 0;
            }
            Ok(PendingEvent::FileChanged {
                path,
                name,
                before,
                after,
            }) => self.record_file_change(path, name, before, after),
            Ok(PendingEvent::CompactFinished(result)) => {
                self.pending = None;
                self.streaming = None;
                self.notice = match result {
                    Ok(()) => "Conversation condensed.".to_owned(),
                    Err(error) => format!("Could not condense the conversation: {error}"),
                };
            }
            Ok(PendingEvent::Subagent(event)) => self.tracker.apply(event),
            Ok(PendingEvent::ApprovalRequest(request)) => {
                self.approval_scroll = 0;
                self.approval_expanded = false;
                self.tool_approval = Some(request);
                self.notice = "The assistant is waiting for your approval.".to_owned();
            }
            Ok(PendingEvent::Finished(Ok(response))) => {
                self.streaming = None;
                self.commit_checkpoint();
                self.messages
                    .push(provider::ChatMessage::assistant(response.text.clone()));
                self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::Assistant,
                    text: response.text.clone(),
                });
                self.history_scroll = 0;
                if response.failed_over {
                    self.settings.active_provider_id = response.provider_id.clone();
                    self.settings.model = Some(response.model_id.clone());
                    if let Some(profile) = response.provider_id.as_deref().and_then(|id| {
                        self.settings
                            .providers
                            .iter()
                            .find(|profile| profile.id == id)
                    }) {
                        self.settings.provider = Some(profile.adapter.clone());
                        self.settings.base_url = profile.base_url.clone();
                    }
                    let _ = write_settings(&self.settings);
                    self.notice = format!(
                        "Usage limit reached; switched to {}.",
                        selected_model_name(&self.settings, &response.model_id)
                    );
                } else {
                    self.notice = "Response received.".to_owned();
                }
                self.pending = None;
            }
            Ok(PendingEvent::Finished(Err(error))) => {
                self.pending = None;
                self.commit_checkpoint();
                if let Some(turn) = self.streaming.take() {
                    self.keep_partial_answer(&turn.text, false);
                }
                let (title, details) = format_provider_error(&error);
                self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::Error,
                    text: format!("{title}\n{details}"),
                });
                self.history_scroll = 0;
                self.notice = format!("Request failed: {title}");
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.streaming = None;
                self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::Error,
                    text: "Request worker stopped unexpectedly.".to_owned(),
                });
                self.history_scroll = 0;
                self.notice = "Request worker stopped unexpectedly.".to_owned();
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    pub(super) fn run_readonly_tool(&self, command: &str) -> String {
        if !self.workspace_trusted {
            return "Tool error: trust this workspace in Settings → Privacy before repository tools can inspect it.".to_owned();
        }
        let root = match std::env::current_dir() {
            Ok(root) => root,
            Err(error) => return format!("Tool error: could not resolve workspace: {error}"),
        };
        let result = if command == "/files" {
            crate::tools::list_files(&root)
        } else if command == "/git status" {
            crate::tools::git_status(&root)
        } else if let Some(path) = command.strip_prefix("/read ") {
            crate::tools::read_file(&root, path.trim())
        } else if let Some(query) = command.strip_prefix("/search ") {
            crate::tools::search(&root, query.trim())
        } else {
            unreachable!("only supported read-only tools are dispatched")
        };
        result.unwrap_or_else(|error| format!("Tool error: {error:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::format_provider_error;
    use crate::agent::PendingEvent;
    use crate::tui::render::draw;
    use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
    use crate::{Settings, provider};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::mpsc;

    fn streaming_app() -> (App, mpsc::Sender<PendingEvent>) {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.streaming = Some(crate::tui::state::StreamingTurn::new(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        (app, sender)
    }

    fn call(id: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "type": "function", "function": {"name": "read_file", "arguments": "{}"}})
    }

    fn unanswered_tool_calls(messages: &[provider::ChatMessage]) -> Vec<String> {
        let answered = messages
            .iter()
            .filter_map(|message| message.tool_call_id.clone())
            .collect::<Vec<_>>();
        messages
            .iter()
            .filter_map(|message| message.tool_calls.as_ref())
            .flatten()
            .filter_map(|call| call.get("id").and_then(|id| id.as_str()))
            .filter(|id| !answered.iter().any(|answered| answered == id))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn cancel_mid_tool_round_leaves_every_tool_call_answered() {
        let (mut app, sender) = streaming_app();
        sender
            .send(PendingEvent::ConversationMessage(
                provider::ChatMessage::assistant_tool_calls(
                    String::new(),
                    vec![call("A"), call("B")],
                ),
            ))
            .unwrap();
        sender
            .send(PendingEvent::ConversationMessage(
                provider::ChatMessage::tool_result(
                    "A".to_owned(),
                    "read_file".to_owned(),
                    "contents".to_owned(),
                ),
            ))
            .unwrap();
        // Nothing has been polled yet: the events are still queued when the user cancels.
        app.cancel_turn();
        assert_eq!(unanswered_tool_calls(&app.messages), Vec::<String>::new());
        let synthetic = app
            .messages
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("B"))
            .expect("synthetic result for B");
        assert!(
            synthetic
                .content
                .as_str()
                .is_some_and(|text| text.to_lowercase().contains("cancelled"))
        );
    }

    fn said(text: &str) -> provider::ChatMessage {
        provider::ChatMessage::assistant(text.to_owned())
    }

    fn asked(text: &str) -> provider::ChatMessage {
        provider::ChatMessage::user_with_images(text.to_owned(), text.to_owned(), Vec::new())
    }

    #[test]
    fn a_compaction_swaps_the_older_messages_for_the_summary_in_the_same_conversation() {
        let (mut app, sender) = streaming_app();
        app.messages = vec![
            asked("one"),
            said("two"),
            asked("three"),
            said("four"),
            asked("five"),
        ];
        sender
            .send(PendingEvent::Compacted {
                replaced: 3,
                with: vec![asked("SUMMARY"), said("Understood.")],
                summary: "Condensed 3 earlier messages.".to_owned(),
            })
            .unwrap();
        // A message that arrives afterwards lands after the summary and what was kept.
        sender
            .send(PendingEvent::ConversationMessage(said("six")))
            .unwrap();
        app.poll_response();
        let texts: Vec<_> = app.messages.iter().map(|m| m.display.as_str()).collect();
        assert_eq!(texts, ["SUMMARY", "Understood.", "four", "five", "six"]);
        assert!(
            app.transcript
                .iter()
                .any(|entry| entry.text == "Condensed 3 earlier messages."),
            "the user is told"
        );
    }

    #[test]
    fn compact_waits_for_a_running_turn_and_needs_enough_conversation() {
        let (mut busy, _sender) = streaming_app();
        busy.input = "/compact".to_owned();
        busy.submit().expect("compact");
        assert!(
            busy.transcript
                .last()
                .unwrap()
                .text
                .contains("Wait for the current turn"),
            "{:?}",
            busy.transcript.last()
        );
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.input = "/compact".to_owned();
        app.submit().expect("compact");
        assert!(app.pending.is_none());
        assert!(
            app.transcript
                .last()
                .unwrap()
                .text
                .contains("not enough conversation"),
            "{:?}",
            app.transcript.last()
        );
    }

    #[test]
    fn compact_starts_a_background_summary_when_there_is_enough_to_condense() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        let big = "z".repeat(12_000);
        for turn in 0..4 {
            app.messages.push(asked(&format!("q{turn} {big}")));
            app.messages.push(said(&format!("a{turn} {big}")));
        }
        app.messages.push(asked("latest"));
        app.input = "/compact".to_owned();
        app.submit().expect("compact");
        assert!(app.pending.is_some(), "the summary is being written");
        assert!(app.streaming.is_some(), "so Esc can cancel it");
        assert!(app.notice.contains("Condensing"), "{}", app.notice);
        // Finishing it frees the prompt again.
        let (sender, receiver) = mpsc::channel();
        sender.send(PendingEvent::CompactFinished(Ok(()))).unwrap();
        app.pending = Some(receiver);
        app.poll_response();
        assert!(app.pending.is_none() && app.streaming.is_none());
        assert_eq!(app.notice, "Conversation condensed.");
    }

    #[test]
    fn the_status_line_shows_how_full_the_context_is() {
        let mut app = App::new(Settings::default());
        assert_eq!(app.context_status(), None, "nothing to show yet");
        app.messages.push(asked(&"a".repeat(40_000)));
        let (text, urgency) = app.context_status().expect("status");
        assert!(
            text.starts_with("ctx 1") && !text.contains('/'),
            "unknown window: {text}"
        );
        assert_eq!(urgency, 0);
        let mut profile = crate::ProviderProfile {
            id: "p".to_owned(),
            ..Default::default()
        };
        profile.model_info.insert(
            "m".to_owned(),
            crate::ModelInfo {
                context: Some(16_000),
                ..Default::default()
            },
        );
        app.settings.providers = vec![profile];
        app.settings.active_provider_id = Some("p".to_owned());
        app.settings.model = Some("m".to_owned());
        let (text, urgency) = app.context_status().expect("status");
        assert!(text.ends_with("/16k"), "{text}");
        assert_eq!(urgency, 1, "above 80%");
        app.messages.push(asked(&"a".repeat(9_000)));
        assert_eq!(
            app.context_status().unwrap().1,
            2,
            "above 95%, and the count follows the conversation"
        );
    }

    fn here() -> std::path::PathBuf {
        std::env::current_dir().unwrap().canonicalize().unwrap()
    }

    #[test]
    fn claudemd_toggles_loading_for_this_project_only() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = true;
        assert!(!app.instruction_files(&here()).project_claude);
        app.input = "/claudemd".to_owned();
        app.submit().expect("on");
        assert!(
            app.notice.contains("CLAUDE.md loading is ON"),
            "{}",
            app.notice
        );
        assert!(app.instruction_files(&here()).project_claude);
        assert!(
            !app.instruction_files(&here()).project_agents,
            "AGENTS.md is separate"
        );
        app.input = "/claudemd".to_owned();
        app.submit().expect("off");
        assert!(app.notice.contains("OFF"), "{}", app.notice);
        assert!(!app.instruction_files(&here()).project_claude);
    }

    #[test]
    fn agentsmd_and_explicit_on_off_arguments_work() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = true;
        for (input, expected) in [
            ("/agentsmd on", true),
            ("/agentsmd on", true),
            ("/agentsmd off", false),
        ] {
            app.input = input.to_owned();
            app.submit().expect("submit");
            assert_eq!(
                app.instruction_files(&here()).project_agents,
                expected,
                "{input}"
            );
        }
        app.input = "/agentsmd maybe".to_owned();
        app.submit().expect("usage");
        assert!(
            app.notice.contains("Usage: /agentsmd [on|off]"),
            "{}",
            app.notice
        );
    }

    #[test]
    fn the_toggle_says_when_it_cannot_take_effect_yet() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = false;
        app.input = "/agentsmd on".to_owned();
        app.submit().expect("on");
        assert!(
            app.notice.contains("once the folder is trusted"),
            "{}",
            app.notice
        );
        assert!(app.notice.contains("no AGENTS.md") || here().join("AGENTS.md").is_file());
    }

    #[test]
    fn a_projects_own_choice_beats_the_global_default() {
        let mut settings = Settings::default();
        settings.default_load_claude_md = true;
        settings.load_global_claude_md = true;
        let mut app = App::new(settings);
        let files = app.instruction_files(&here());
        assert!(files.project_claude && files.global_claude && !files.project_agents);
        app.input = "/claudemd off".to_owned();
        app.submit().expect("off");
        let files = app.instruction_files(&here());
        assert!(!files.project_claude, "this project opted out");
        assert!(files.global_claude, "the global file is unaffected");
    }

    #[test]
    fn the_system_prompt_names_the_policy_and_the_permission_mode() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        let prompt = app.build_system_prompt().expect("prompt");
        assert!(prompt.contains("Built-in harness policy"), "{prompt}");
        assert!(prompt.contains("Permission mode:"), "{prompt}");
        assert!(prompt.contains("Working folder:"), "{prompt}");
    }

    #[test]
    fn super_and_ultimate_are_locked_until_dynamic_workflows_are_on() {
        for name in ["super", "ultimate", "extreme"] {
            let mut app = App::new(Settings::default());
            app.trust_prompt = false;
            app.settings.ultimate_acknowledged = true;
            let before = app.settings.effort;
            app.input = format!("/effort {name}");
            app.submit().expect("submit");
            assert_eq!(app.settings.effort, before, "{name} must not apply");
            assert!(app.notice.contains("locked"), "{name}: {}", app.notice);
            assert!(
                !app.confirm_ultimate,
                "no confirmation for something locked"
            );
            assert!(!app.picker);
        }
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.input = "/effort max".to_owned();
        app.submit().expect("max");
        assert_eq!(
            app.settings.effort,
            crate::Effort::Max,
            "ordinary levels stay open"
        );
    }

    #[test]
    fn a_saved_workflow_tier_is_pulled_back_when_workflows_are_locked() {
        let mut settings = Settings::default();
        settings.effort = crate::Effort::Ultimate;
        let app = App::new(settings);
        assert_eq!(app.settings.effort, crate::Effort::Max);
        assert!(app.notice.contains("locked"), "{}", app.notice);
        let mut unlocked = Settings::default();
        unlocked.effort = crate::Effort::Super;
        unlocked.workflow_size = crate::workflow::WorkflowSize::Medium;
        assert_eq!(App::new(unlocked).settings.effort, crate::Effort::Super);
    }

    #[test]
    fn effort_accepts_ultimate_and_its_old_name() {
        for name in ["ultimate", "extreme"] {
            let mut app = App::new(Settings::default());
            app.trust_prompt = false;
            app.settings.ultimate_acknowledged = true;
            app.settings.workflow_size = crate::workflow::WorkflowSize::Medium;
            app.input = format!("/effort {name}");
            app.submit().expect("submit");
            assert_eq!(app.settings.effort, crate::Effort::Ultimate, "{name}");
        }
    }

    #[test]
    fn a_finished_turn_is_saved_as_a_session() {
        let (mut app, sender) = streaming_app();
        app.settings.sessions_enabled = true;
        app.messages.push(provider::ChatMessage::user_with_images(
            "write a parser".to_owned(),
            "write a parser".to_owned(),
            Vec::new(),
        ));
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: "write a parser".to_owned(),
        });
        sender
            .send(PendingEvent::Finished(Ok(provider::Completion {
                text: "Done.".to_owned(),
                provider_id: None,
                model_id: "m".to_owned(),
                failed_over: false,
                tool_calls: Vec::new(),
            })))
            .unwrap();
        assert!(crate::session::list_in(&app.session_dir, None).is_empty());
        app.poll_response();
        let saved = crate::session::load_in(&app.session_dir, &app.session_id).expect("saved");
        assert_eq!(saved.messages.len(), 2);
        assert_eq!(saved.header.title, "write a parser");
        assert_eq!(
            saved.transcript.last().map(|e| e.text.as_str()),
            Some("Done.")
        );
    }

    #[test]
    fn a_failed_turn_is_saved_too() {
        let (mut app, sender) = streaming_app();
        app.settings.sessions_enabled = true;
        app.messages.push(provider::ChatMessage::user_with_images(
            "try this".to_owned(),
            "try this".to_owned(),
            Vec::new(),
        ));
        sender
            .send(PendingEvent::Finished(Err("boom".to_owned())))
            .unwrap();
        app.poll_response();
        assert_eq!(crate::session::list_in(&app.session_dir, None).len(), 1);
    }

    #[test]
    fn sending_a_prompt_saves_it_before_the_answer_arrives() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.settings.sessions_enabled = true;
        app.input = "hello there".to_owned();
        app.submit().expect("submit");
        let listed = crate::session::list_in(&app.session_dir, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "hello there");
    }

    #[test]
    fn cancel_after_the_turn_already_finished_keeps_the_final_answer() {
        let (mut app, sender) = streaming_app();
        sender
            .send(PendingEvent::TextDelta("Done.".to_owned()))
            .unwrap();
        sender
            .send(PendingEvent::Finished(Ok(provider::Completion {
                text: "Done.".to_owned(),
                provider_id: None,
                model_id: "m".to_owned(),
                failed_over: false,
                tool_calls: Vec::new(),
            })))
            .unwrap();
        app.cancel_turn();
        assert!(app.streaming.is_none());
        assert_eq!(
            app.transcript.last().map(|entry| entry.text.as_str()),
            Some("Done.")
        );
        assert!(
            app.transcript
                .iter()
                .all(|entry| entry.text != "(interrupted)")
        );
    }

    #[test]
    fn a_failure_without_partial_text_is_not_marked_interrupted() {
        let (mut app, sender) = streaming_app();
        sender
            .send(PendingEvent::Finished(Err(
                "agent tool-call round limit reached".to_owned(),
            )))
            .unwrap();
        app.poll_response();
        assert!(
            app.transcript
                .iter()
                .all(|entry| entry.text != "(interrupted)")
        );
        assert_eq!(
            app.transcript.last().map(|entry| entry.kind),
            Some(TranscriptKind::Error)
        );
    }

    #[test]
    fn cancelling_before_any_text_still_says_interrupted() {
        let (mut app, _sender) = streaming_app();
        app.cancel_turn();
        assert!(
            app.transcript
                .iter()
                .any(|entry| entry.text == "(interrupted)")
        );
    }

    #[test]
    fn a_rejected_key_in_chat_says_how_to_replace_it() {
        let (status, details) = super::format_provider_error(
            r#"provider returned 401 Unauthorized: {"error":{"code":"invalid_credential","message":"Invalid or missing credential."}}"#,
        );
        assert!(status.starts_with("401"), "{status}");
        assert!(
            details.contains("Invalid or missing credential."),
            "{details}"
        );
        assert!(
            details.contains("press e") && details.contains("/provider"),
            "{details}"
        );
        let (_, rate_limited) = super::format_provider_error(
            r#"provider returned 429 Too Many Requests: {"error":{"message":"slow down"}}"#,
        );
        assert_eq!(rate_limited, "slow down");
    }

    #[test]
    fn slash_stats_opens_the_view_and_stats_clear_asks_first() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.input = "/stats".to_owned();
        app.submit().expect("stats");
        let view = app.stats_view.as_ref().expect("view open");
        assert!(!view.confirm_clear);
        app.stats_view = None;
        app.input = "/stats clear".to_owned();
        app.submit().expect("clear");
        assert!(app.stats_view.as_ref().expect("view open").confirm_clear);
        app.input = "/help".to_owned();
        app.stats_view = None;
        app.submit().expect("help");
        assert!(app.notice.contains("/stats"), "{}", app.notice);
    }

    #[test]
    fn arrivals_older_than_the_pulse_are_pruned() {
        let (mut app, _sender) = streaming_app();
        let turn = app.streaming.as_mut().expect("streaming");
        let now = std::time::Instant::now();
        turn.arrivals = vec![
            (0, now - std::time::Duration::from_secs(5)),
            (10, now - std::time::Duration::from_secs(2)),
            (20, now - std::time::Duration::from_millis(100)),
        ];
        turn.prune_arrivals(now);
        assert_eq!(turn.arrivals.len(), 1);
        assert_eq!(turn.arrivals[0].0, 20);
    }

    #[test]
    fn poll_drains_all_waiting_events() {
        let (mut app, sender) = streaming_app();
        for piece in ["a", "b", "c"] {
            sender
                .send(PendingEvent::TextDelta(piece.to_owned()))
                .unwrap();
        }
        sender.send(PendingEvent::Usage(3)).unwrap();
        app.poll_response();
        let turn = app.streaming.as_ref().expect("streaming");
        assert_eq!(turn.text, "abc");
        assert_eq!(turn.usage, Some(3));
        assert_eq!(turn.arrivals.len(), 3);
    }

    #[test]
    fn tool_round_text_moves_into_transcript() {
        let (mut app, sender) = streaming_app();
        sender
            .send(PendingEvent::TextDelta("Let me check.".to_owned()))
            .unwrap();
        sender
            .send(PendingEvent::ConversationMessage(
                provider::ChatMessage::assistant_tool_calls("Let me check.".to_owned(), Vec::new()),
            ))
            .unwrap();
        sender
            .send(PendingEvent::ToolStarted("cargo test".to_owned()))
            .unwrap();
        app.poll_response();
        assert_eq!(
            app.transcript.last().map(|entry| entry.text.as_str()),
            Some("Let me check.")
        );
        let turn = app.streaming.as_ref().expect("streaming");
        assert!(turn.text.is_empty());
        assert_eq!(
            turn.tool.as_ref().map(|(label, _)| label.as_str()),
            Some("cargo test")
        );
    }

    #[test]
    fn finished_turn_replaces_streaming_with_final_entry() {
        let (mut app, sender) = streaming_app();
        sender
            .send(PendingEvent::TextDelta("Done.".to_owned()))
            .unwrap();
        sender
            .send(PendingEvent::Finished(Ok(provider::Completion {
                text: "Done.".to_owned(),
                provider_id: None,
                model_id: "m".to_owned(),
                failed_over: false,
                tool_calls: Vec::new(),
            })))
            .unwrap();
        app.poll_response();
        assert!(app.streaming.is_none());
        assert!(app.pending.is_none());
        assert_eq!(
            app.transcript.last().map(|entry| entry.text.as_str()),
            Some("Done.")
        );
    }

    #[test]
    fn slash_command_is_rendered_as_user_input_and_plain_command_output() {
        let mut app = App::new(Settings::default());
        app.input = "/help".to_owned();
        app.submit().expect("run help command");
        assert_eq!(app.transcript.len(), 2);
        assert_eq!(app.transcript[0].kind, TranscriptKind::User);
        assert_eq!(app.transcript[0].text, "/help");
        assert_eq!(app.transcript[1].kind, TranscriptKind::CommandOutput);
        assert!(app.transcript[1].text.contains("/files"));

        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        app.trust_prompt = false;
        terminal
            .draw(|frame| draw(frame, &app, 0))
            .expect("draw command transcript");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Commands:"));
    }

    #[test]
    fn clear_empties_the_transcript_and_restores_the_welcome_layout() {
        let mut app = App::new(Settings::default());
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Assistant,
            text: "old conversation".to_owned(),
        });
        app.messages.push(provider::ChatMessage::assistant(
            "old conversation".to_owned(),
        ));
        app.input = "/clear".to_owned();
        app.submit().expect("clear conversation");
        assert!(app.transcript.is_empty());
        assert!(app.messages.is_empty());
    }

    #[test]
    fn completed_answer_returns_history_to_bottom() {
        let mut app = App::new(Settings::default());
        app.history_scroll = 12;
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender.send(PendingEvent::ConversationMessage(provider::ChatMessage::assistant_tool_calls(
            String::new(),
            vec![serde_json::json!({"id":"call-1", "type":"function", "function":{"name":"read_file", "arguments":"{}"}})],
        ))).expect("send tool call history");
        sender
            .send(PendingEvent::ConversationMessage(
                provider::ChatMessage::tool_result(
                    "call-1".to_owned(),
                    "read_file".to_owned(),
                    "file was read".to_owned(),
                ),
            ))
            .expect("send tool result history");
        sender
            .send(PendingEvent::Finished(Ok(provider::Completion {
                text: "done".to_owned(),
                provider_id: None,
                model_id: "test-model".to_owned(),
                failed_over: false,
                tool_calls: Vec::new(),
            })))
            .expect("send mock response");
        app.poll_response();
        app.poll_response();
        app.poll_response();
        assert_eq!(app.history_scroll, 0);
        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            ["assistant", "tool", "assistant"]
        );
    }

    #[test]
    fn provider_errors_are_transcript_output_with_status_and_api_message() {
        let raw_error = r#"Gemini remained unavailable: provider returned 503 Service Unavailable: {"error":{"code":503,"message":"This model is currently experiencing high demand. Please try again later."}}"#;
        let (status, message) = format_provider_error(raw_error);
        assert_eq!(status, "503 Service Unavailable");
        assert_eq!(
            message,
            "This model is currently experiencing high demand. Please try again later."
        );

        let mut app = App::new(Settings::default());
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(PendingEvent::Finished(Err(raw_error.to_owned())))
            .expect("send mock provider error");
        app.poll_response();
        assert_eq!(app.transcript.len(), 1);
        assert_eq!(app.transcript[0].kind, TranscriptKind::Error);
        assert!(app.transcript[0].text.contains("503 Service Unavailable"));
        assert!(
            app.transcript[0]
                .text
                .contains("currently experiencing high demand")
        );
    }
}
