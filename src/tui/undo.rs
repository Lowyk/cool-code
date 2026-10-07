//! `/undo`: taking back the file changes the model made in its last turn.
//!
//! Every successful edit or file creation reports the file's text before and after. One turn's
//! changes form a checkpoint. Undoing a checkpoint restores each file only if it is still exactly
//! as the turn left it, so nothing written since (by you or by a command) is overwritten.
//! Changes made by `run_command` are not tracked.

use crate::provider::ChatMessage;
use crate::tui::state::App;
use serde_json::Value;
use std::path::PathBuf;

/// How many turns can be undone.
const MAX_CHECKPOINTS: usize = 20;
/// How much of the request is kept as the checkpoint's name.
const LABEL_CHARS: usize = 50;

pub(in crate::tui) struct FileChange {
    path: PathBuf,
    /// The path as the model named it, for messages.
    name: String,
    /// The text before the turn's first change; `None` when the turn created the file.
    before: Option<String>,
    /// The text after the turn's last change.
    after: String,
}

pub(in crate::tui) struct Checkpoint {
    label: String,
    files: Vec<FileChange>,
}

/// Puts a note in front of the user's next message, so the model learns about an undo.
pub(super) fn with_note(mut message: ChatMessage, note: &str) -> ChatMessage {
    let prefix = format!("[Note from the harness: {note}]\n\n");
    match &mut message.content {
        Value::String(text) => text.insert_str(0, &prefix),
        Value::Array(parts) => {
            if let Some(Value::String(text)) =
                parts.first_mut().and_then(|part| part.get_mut("text"))
            {
                text.insert_str(0, &prefix);
            }
        }
        _ => {}
    }
    message
}

impl App {
    /// Remembers a file the model changed in the current turn. The first `before` and the latest
    /// `after` of a file are kept.
    pub(in crate::tui) fn record_file_change(
        &mut self,
        path: PathBuf,
        name: String,
        before: Option<String>,
        after: String,
    ) {
        match self.journal.iter_mut().find(|file| file.path == path) {
            Some(file) => file.after = after,
            None => self.journal.push(FileChange {
                path,
                name,
                before,
                after,
            }),
        }
    }

    /// Closes the current turn's journal into a checkpoint (when it holds anything).
    pub(in crate::tui) fn commit_checkpoint(&mut self) {
        if self.journal.is_empty() {
            return;
        }
        let request = self
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.display.replace('\n', " "))
            .unwrap_or_default();
        let label: String = request.chars().take(LABEL_CHARS).collect();
        self.checkpoints.push(Checkpoint {
            label,
            files: std::mem::take(&mut self.journal),
        });
        if self.checkpoints.len() > MAX_CHECKPOINTS {
            self.checkpoints.remove(0);
        }
    }

    /// Takes back the most recent checkpoint; returns what to tell the user.
    pub(super) fn undo_last(&mut self) -> String {
        if self.pending.is_some() {
            return "Wait for the current turn to finish before undoing.".to_owned();
        }
        let Some(checkpoint) = self.checkpoints.pop() else {
            return "Nothing to undo. Only file changes the model made through its edit tools are tracked."
                .to_owned();
        };
        let mut restored = Vec::new();
        let mut removed = Vec::new();
        let mut left = Vec::new();
        for file in checkpoint.files.iter().rev() {
            let current = std::fs::read_to_string(&file.path).ok();
            if current.as_deref() != Some(file.after.as_str()) {
                left.push(file.name.clone());
                continue;
            }
            let outcome = match &file.before {
                Some(text) => std::fs::write(&file.path, text).map(|()| restored.push(&file.name)),
                None => std::fs::remove_file(&file.path).map(|()| removed.push(&file.name)),
            };
            if outcome.is_err() {
                left.push(file.name.clone());
            }
        }
        let mut parts = Vec::new();
        if !restored.is_empty() {
            parts.push(format!(
                "restored {}",
                restored
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !removed.is_empty() {
            parts.push(format!(
                "removed {}",
                removed
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let mut message = if parts.is_empty() {
            "Nothing was changed".to_owned()
        } else {
            format!("Undid “{}”: {}", checkpoint.label, parts.join("; "))
        };
        if !left.is_empty() {
            message.push_str(&format!(
                ". Left alone because they changed since: {}",
                left.join(", ")
            ));
        }
        message.push('.');
        if !restored.is_empty() || !removed.is_empty() {
            self.pending_note = Some(format!(
                "the user undid the file changes from the earlier request “{}”: {}. The files are back as they were; do not assume those edits exist.",
                checkpoint.label,
                parts.join("; ")
            ));
        }
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;

    fn workspace() -> PathBuf {
        let root = std::env::temp_dir().join(format!("harness-undo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn app_with_a_request(request: &str) -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.messages.push(ChatMessage::user_with_images(
            request.to_owned(),
            request.to_owned(),
            Vec::new(),
        ));
        app
    }

    /// What the model's edit tools did: change `name` from `before` to `after` on disk and report it.
    fn edit(app: &mut App, root: &std::path::Path, name: &str, before: Option<&str>, after: &str) {
        let path = root.join(name);
        std::fs::write(&path, after).unwrap();
        app.record_file_change(
            path,
            name.to_owned(),
            before.map(str::to_owned),
            after.to_owned(),
        );
    }

    #[test]
    fn undo_restores_edited_files_and_removes_created_ones() {
        let root = workspace();
        std::fs::write(root.join("a.txt"), "original").unwrap();
        let mut app = app_with_a_request("make the change");
        edit(&mut app, &root, "a.txt", Some("original"), "edited");
        edit(&mut app, &root, "new.txt", None, "fresh");
        app.commit_checkpoint();
        let message = app.undo_last();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "original"
        );
        assert!(!root.join("new.txt").exists());
        assert!(message.contains("make the change"), "{message}");
        assert!(
            message.contains("restored a.txt") && message.contains("removed new.txt"),
            "{message}"
        );
        assert!(
            app.pending_note
                .as_deref()
                .is_some_and(|note| note.contains("undid"))
        );
    }

    #[test]
    fn several_edits_to_one_file_undo_to_the_state_before_the_turn() {
        let root = workspace();
        std::fs::write(root.join("a.txt"), "v0").unwrap();
        let mut app = app_with_a_request("iterate");
        edit(&mut app, &root, "a.txt", Some("v0"), "v1");
        edit(&mut app, &root, "a.txt", Some("v1"), "v2");
        app.commit_checkpoint();
        app.undo_last();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "v0");
    }

    #[test]
    fn a_file_changed_since_is_left_alone() {
        let root = workspace();
        std::fs::write(root.join("a.txt"), "original").unwrap();
        std::fs::write(root.join("b.txt"), "b0").unwrap();
        let mut app = app_with_a_request("two files");
        edit(&mut app, &root, "a.txt", Some("original"), "by the model");
        edit(&mut app, &root, "b.txt", Some("b0"), "b1");
        app.commit_checkpoint();
        std::fs::write(root.join("a.txt"), "by the user afterwards").unwrap();
        let message = app.undo_last();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "by the user afterwards",
            "the user's later work is never overwritten"
        );
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "b0");
        assert!(
            message.contains("Left alone because they changed since: a.txt"),
            "{message}"
        );
    }

    #[test]
    fn undo_goes_back_one_turn_at_a_time_and_says_when_there_is_nothing_left() {
        let root = workspace();
        std::fs::write(root.join("a.txt"), "v0").unwrap();
        let mut app = app_with_a_request("first");
        edit(&mut app, &root, "a.txt", Some("v0"), "v1");
        app.commit_checkpoint();
        app.messages.push(ChatMessage::user_with_images(
            "second".to_owned(),
            "second".to_owned(),
            Vec::new(),
        ));
        edit(&mut app, &root, "a.txt", Some("v1"), "v2");
        app.commit_checkpoint();
        assert!(app.undo_last().contains("second"));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "v1");
        assert!(app.undo_last().contains("first"));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "v0");
        assert!(app.undo_last().starts_with("Nothing to undo"));
    }

    #[test]
    fn a_turn_that_changed_nothing_leaves_no_checkpoint() {
        let mut app = app_with_a_request("just a question");
        app.commit_checkpoint();
        assert!(app.checkpoints.is_empty());
    }

    #[test]
    fn only_the_most_recent_checkpoints_are_kept() {
        let root = workspace();
        let mut app = app_with_a_request("many");
        for turn in 0..MAX_CHECKPOINTS + 5 {
            edit(&mut app, &root, "a.txt", None, &format!("v{turn}"));
            app.commit_checkpoint();
        }
        assert_eq!(app.checkpoints.len(), MAX_CHECKPOINTS);
    }

    #[test]
    fn a_turns_changes_become_a_checkpoint_when_it_finishes_and_the_command_undoes_them() {
        use crate::agent::PendingEvent;
        let root = workspace();
        std::fs::write(root.join("a.txt"), "v0").unwrap();
        let mut app = app_with_a_request("do it");
        let (sender, receiver) = std::sync::mpsc::channel();
        app.pending = Some(receiver);
        std::fs::write(root.join("a.txt"), "v1").unwrap();
        sender
            .send(PendingEvent::FileChanged {
                path: root.join("a.txt"),
                name: "a.txt".to_owned(),
                before: Some("v0".to_owned()),
                after: "v1".to_owned(),
            })
            .unwrap();
        sender
            .send(PendingEvent::Finished(Ok(crate::provider::Completion {
                text: "done".to_owned(),
                provider_id: None,
                model_id: "m".to_owned(),
                failed_over: false,
                tool_calls: Vec::new(),
            })))
            .unwrap();
        app.poll_response();
        assert_eq!(app.checkpoints.len(), 1, "the turn's changes were kept");
        assert!(app.journal.is_empty());
        app.input = "/undo".to_owned();
        app.submit().expect("undo");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "v0");
        assert!(
            app.transcript
                .last()
                .unwrap()
                .text
                .contains("restored a.txt")
        );
        assert!(app.notice.contains("restored a.txt"), "{}", app.notice);
    }

    #[test]
    fn undo_waits_for_a_running_turn() {
        let mut app = app_with_a_request("busy");
        let (_sender, receiver) = std::sync::mpsc::channel();
        app.pending = Some(receiver);
        assert!(app.undo_last().contains("Wait for the current turn"));
    }

    #[test]
    fn the_model_is_told_about_an_undo_in_front_of_the_next_message() {
        let message =
            ChatMessage::user_with_images("continue".to_owned(), "continue".to_owned(), Vec::new());
        let noted = with_note(message, "the user undid something");
        assert_eq!(
            noted.content.as_str().unwrap(),
            "[Note from the harness: the user undid something]\n\ncontinue"
        );
        assert_eq!(noted.display, "continue", "what the user sees is unchanged");
        let with_image = ChatMessage::user_with_images(
            "look".to_owned(),
            "look".to_owned(),
            vec![serde_json::json!({"type": "image_url", "image_url": {"url": "data:x"}})],
        );
        let noted = with_note(with_image, "n");
        assert!(
            noted.content[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("[Note from the harness: n]")
        );
        assert_eq!(
            noted.content.as_array().unwrap().len(),
            2,
            "the image is kept"
        );
    }
}
