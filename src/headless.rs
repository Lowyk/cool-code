//! `coolcode run`: one turn without the interface, for scripts and CI.
//!
//! The answer goes to standard output; progress goes to standard error (or, with `--json`,
//! everything is one JSON object per line on standard output). There is nobody to ask, so any
//! approval the permission mode would ask for is declined: choose a mode that fits the job
//! (`--mode accept-edits`, for example) rather than expecting a prompt.

use crate::agent::{PendingEvent, run_agent_turns};
use crate::policy::MODES;
use crate::provider::{self, Completion};
use crate::{Effort, Settings, read_settings};
use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Read, Write};
use std::sync::mpsc::{self, Receiver};

pub(crate) struct Options {
    /// What to ask; read from standard input when empty.
    pub(crate) prompt: String,
    pub(crate) model: Option<String>,
    pub(crate) mode: Option<String>,
    pub(crate) effort: Option<Effort>,
    /// Treat this folder as trusted for this run only (nothing is saved).
    pub(crate) trust: bool,
    pub(crate) json: bool,
}

/// Applies the command-line choices on top of the saved settings.
pub(crate) fn apply_overrides(settings: &mut Settings, options: &Options) -> Result<()> {
    if let Some(mode) = &options.mode {
        if !MODES.iter().any(|(_, name)| name == mode) {
            let names = MODES
                .iter()
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            bail!("unknown mode `{mode}`; choose one of: {names}");
        }
        settings.permission_mode = mode.clone();
    }
    if settings.permission_mode == "auto" && !settings.auto_ready() {
        bail!(
            "auto mode needs a judge model: choose one in the interactive harness under Settings > Auto Mode, or pick another --mode"
        );
    }
    if let Some(model) = &options.model {
        if model.trim().is_empty() {
            bail!("--model needs a model id");
        }
        settings.model = Some(model.trim().to_owned());
    }
    if let Some(effort) = options.effort {
        settings.effort = effort;
    }
    // The workflow tiers stay locked here exactly as they do in the interface.
    settings.enforce_workflow_lock();
    Ok(())
}

/// Refuses a provider that needs a one-time privacy acknowledgement nobody has given.
fn check_privacy(settings: &Settings, message: &provider::ChatMessage) -> Result<()> {
    let Some(risk) = provider::privacy_risk_for_settings(settings) else {
        return Ok(());
    };
    if !settings.privacy_acknowledged.iter().any(|ack| ack == risk) {
        bail!(
            "{risk} needs a one-time acknowledgement: send a message to this provider in the interactive harness first"
        );
    }
    if provider::message_contains_image(message)
        && !settings
            .privacy_image_acknowledged
            .iter()
            .any(|ack| ack == risk)
    {
        bail!("image contents for {risk} have not been authorized in the interactive harness");
    }
    Ok(())
}

fn write_json(out: &mut dyn Write, value: serde_json::Value) {
    let _ = writeln!(out, "{value}");
}

/// Reads the worker's events until it finishes, showing progress and declining approvals.
pub(crate) fn consume(
    receiver: &Receiver<PendingEvent>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    json: bool,
) -> Result<Completion> {
    loop {
        let event = receiver
            .recv()
            .context("the model worker stopped unexpectedly")?;
        match event {
            PendingEvent::ToolAction(action) => {
                if json {
                    write_json(out, serde_json::json!({"type": "tool", "text": action}));
                } else {
                    let _ = writeln!(err, "• {action}");
                }
            }
            PendingEvent::ApprovalRequest(request) => {
                let _ = request.response.send(false);
                if json {
                    write_json(
                        out,
                        serde_json::json!({"type": "declined", "text": request.title}),
                    );
                } else {
                    let _ = writeln!(
                        err,
                        "• declined (nobody to ask in this mode): {}",
                        request.title
                    );
                }
            }
            PendingEvent::Compacted { summary, .. } => {
                if json {
                    write_json(out, serde_json::json!({"type": "note", "text": summary}));
                } else {
                    let _ = writeln!(err, "• {summary}");
                }
            }
            PendingEvent::FileChanged { name, .. } => {
                if json {
                    write_json(out, serde_json::json!({"type": "file", "path": name}));
                }
            }
            PendingEvent::Finished(Ok(completion)) => return Ok(completion),
            PendingEvent::Finished(Err(error)) => bail!("{error}"),
            PendingEvent::TextDelta(_)
            | PendingEvent::Usage(_)
            | PendingEvent::ToolStarted(_)
            | PendingEvent::ConversationMessage(_)
            | PendingEvent::CompactFinished(_) => {}
        }
    }
}

fn read_prompt(given: &str) -> Result<String> {
    let given = given.trim();
    if !given.is_empty() && given != "-" {
        return Ok(given.to_owned());
    }
    if std::io::stdin().is_terminal() {
        bail!("give a prompt: coolcode run \"what to do\", or pipe it in on standard input");
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .context("reading the prompt from standard input")?;
    if text.trim().is_empty() {
        bail!("the prompt is empty");
    }
    Ok(text.trim().to_owned())
}

pub(crate) fn run(options: Options) -> Result<()> {
    let mut settings = read_settings()?;
    apply_overrides(&mut settings, &options)?;
    let prompt = read_prompt(&options.prompt)?;
    let root = std::env::current_dir()?
        .canonicalize()
        .context("resolving the workspace folder")?;
    let projects_path = crate::projects::default_path();
    let trusted = options.trust || crate::projects::is_trusted_at(&projects_path, &root);
    let user_message = crate::tui::context::build_user_message(&prompt, trusted)?;
    check_privacy(&settings, &user_message)?;
    let (system_prompt, warnings) =
        crate::prompt::assemble(&settings, trusted, &root, &projects_path)?;
    let mut err = std::io::stderr();
    for warning in warnings {
        let _ = writeln!(err, "• skipped an instruction file: {warning}");
    }
    if !trusted {
        let _ = writeln!(
            err,
            "• this folder is not trusted, so the model has no tools here (use --trust for this run, or trust it in the interface)"
        );
    }
    let messages = vec![provider::ChatMessage::system(system_prompt), user_message];
    let (sender, receiver) = mpsc::channel();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = std::thread::spawn(move || {
        let result = run_agent_turns(settings, messages, root, trusted, &sender, cancel)
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send(PendingEvent::Finished(result));
    });
    let mut out = std::io::stdout();
    let completion = consume(&receiver, &mut out, &mut err, options.json);
    let _ = worker.join();
    let completion = completion?;
    if options.json {
        write_json(
            &mut out,
            serde_json::json!({"type": "result", "text": completion.text}),
        );
    } else {
        let _ = writeln!(out, "{}", completion.text);
    }
    let _ = out.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolApproval;

    fn options() -> Options {
        Options {
            prompt: "do it".to_owned(),
            model: None,
            mode: None,
            effort: None,
            trust: false,
            json: false,
        }
    }

    fn finished(text: &str) -> PendingEvent {
        PendingEvent::Finished(Ok(Completion {
            text: text.to_owned(),
            provider_id: None,
            model_id: "m".to_owned(),
            failed_over: false,
            tool_calls: Vec::new(),
        }))
    }

    fn consume_events(
        events: Vec<PendingEvent>,
        json: bool,
    ) -> (Result<Completion>, String, String) {
        let (sender, receiver) = mpsc::channel();
        for event in events {
            sender.send(event).unwrap();
        }
        // Nothing more is coming, so a missing Finished is noticed instead of waited for.
        drop(sender);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let result = consume(&receiver, &mut out, &mut err, json);
        (
            result,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn command_line_choices_override_the_saved_settings() {
        let mut settings = Settings::default();
        settings.permission_mode = "plan".to_owned();
        let chosen = Options {
            model: Some(" my-model ".to_owned()),
            mode: Some("accept-edits".to_owned()),
            effort: Some(Effort::Low),
            ..options()
        };
        apply_overrides(&mut settings, &chosen).unwrap();
        assert_eq!(settings.model.as_deref(), Some("my-model"));
        assert_eq!(settings.permission_mode, "accept-edits");
        assert_eq!(settings.effort, Effort::Low);
        let mut untouched = Settings::default();
        let before = (untouched.permission_mode.clone(), untouched.effort);
        apply_overrides(&mut untouched, &options()).unwrap();
        assert_eq!((untouched.permission_mode, untouched.effort), before);
    }

    #[test]
    fn auto_mode_needs_a_judge_model_and_manual_is_always_allowed() {
        let auto = Options {
            mode: Some("auto".to_owned()),
            ..options()
        };
        let mut settings = Settings::default();
        let error = apply_overrides(&mut settings, &auto)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Settings > Auto Mode"), "{error}");
        let mut saved_auto = Settings::default();
        saved_auto.permission_mode = "auto".to_owned();
        assert!(
            apply_overrides(&mut saved_auto, &options()).is_err(),
            "a saved Auto mode without a judge is refused too"
        );
        let manual = Options {
            mode: Some("manual".to_owned()),
            ..options()
        };
        apply_overrides(&mut Settings::default(), &manual).unwrap();
        let mut ready = Settings::default();
        ready.providers = vec![crate::ProviderProfile {
            id: "p".to_owned(),
            name: "p".to_owned(),
            adapter: "openai-compatible".to_owned(),
            ..Default::default()
        }];
        ready.auto_guards = vec![crate::guard::AutoGuard {
            provider_id: "p".to_owned(),
            model_id: "haiku".to_owned(),
        }];
        apply_overrides(&mut ready, &auto).unwrap();
        assert_eq!(ready.permission_mode, "auto");
    }

    #[test]
    fn an_unknown_mode_or_empty_model_is_refused_with_the_choices() {
        let mut settings = Settings::default();
        let error = apply_overrides(
            &mut settings,
            &Options {
                mode: Some("yolo".to_owned()),
                ..options()
            },
        )
        .unwrap_err();
        let text = format!("{error}");
        assert!(
            text.contains("unknown mode `yolo`") && text.contains("accept-edits"),
            "{text}"
        );
        assert!(
            apply_overrides(
                &mut settings,
                &Options {
                    model: Some("  ".to_owned()),
                    ..options()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn the_workflow_tiers_stay_locked_unless_dynamic_workflows_are_on() {
        let mut locked = Settings::default();
        apply_overrides(
            &mut locked,
            &Options {
                effort: Some(Effort::Ultimate),
                ..options()
            },
        )
        .unwrap();
        assert_eq!(
            locked.effort,
            Effort::Max,
            "Ultimate becomes Max while locked"
        );
        let mut unlocked = Settings::default();
        unlocked.dynamic_workflows = true;
        apply_overrides(
            &mut unlocked,
            &Options {
                effort: Some(Effort::Ultimate),
                ..options()
            },
        )
        .unwrap();
        assert_eq!(unlocked.effort, Effort::Ultimate);
    }

    #[test]
    fn progress_goes_to_standard_error_and_the_answer_is_returned() {
        let (result, out, err) = consume_events(
            vec![
                PendingEvent::ToolAction("Tool · read_file · 3 lines".to_owned()),
                PendingEvent::TextDelta("partial".to_owned()),
                finished("the answer"),
            ],
            false,
        );
        assert_eq!(result.unwrap().text, "the answer");
        assert!(out.is_empty(), "standard output is only the answer: {out}");
        assert_eq!(err, "• Tool · read_file · 3 lines\n");
    }

    #[test]
    fn an_approval_nobody_can_give_is_declined_and_reported() {
        let (answer_sender, answer) = mpsc::sync_channel(1);
        let (result, _, err) = consume_events(
            vec![
                PendingEvent::ApprovalRequest(ToolApproval {
                    title: "Run shell command".to_owned(),
                    details: "rm -rf build".to_owned(),
                    response: answer_sender,
                }),
                finished("ok"),
            ],
            false,
        );
        assert!(result.is_ok());
        assert!(!answer.recv().unwrap(), "never approved on its own");
        assert!(
            err.contains("declined") && err.contains("Run shell command"),
            "{err}"
        );
    }

    #[test]
    fn json_mode_writes_one_object_per_line() {
        let (result, out, err) = consume_events(
            vec![
                PendingEvent::ToolAction("Tool · list_files · 4 entries".to_owned()),
                PendingEvent::FileChanged {
                    path: "/x/a.txt".into(),
                    name: "a.txt".to_owned(),
                    before: None,
                    after: crate::agent::FileContent::Text("x".to_owned()),
                },
                finished("done"),
            ],
            true,
        );
        assert!(result.is_ok());
        assert!(err.is_empty(), "{err}");
        let lines: Vec<serde_json::Value> = out
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is JSON"))
            .collect();
        assert_eq!(lines[0]["type"], "tool");
        assert_eq!(lines[1]["type"], "file");
        assert_eq!(lines[1]["path"], "a.txt");
    }

    #[test]
    fn a_failed_turn_is_an_error_and_a_vanished_worker_is_too() {
        let (result, _, _) = consume_events(
            vec![PendingEvent::Finished(Err(
                "provider returned 401".to_owned()
            ))],
            false,
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("401"));
        let (result, _, _) = consume_events(Vec::new(), false);
        assert!(format!("{:#}", result.unwrap_err()).contains("worker stopped"));
    }

    #[test]
    fn a_prompt_given_on_the_command_line_is_used_as_is() {
        assert_eq!(read_prompt("  fix the bug  ").unwrap(), "fix the bug");
    }
}
