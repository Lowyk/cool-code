//! Earlier prompts, recalled with ↑ and ↓ in an empty prompt. They are kept between runs in
//! `~/.coolcode/history.jsonl`, but only while Save sessions is on: a prompt can hold anything,
//! so nothing is written to disk unless the user already agreed to keep their conversations.

use crate::tui::state::App;
use std::path::{Path, PathBuf};

/// Most prompts kept.
const MAX_ENTRIES: usize = 500;

#[derive(Default)]
pub(in crate::tui) struct PromptHistory {
    entries: Vec<String>,
    /// Which entry is on screen while browsing.
    position: Option<usize>,
    /// What was in the prompt before browsing began.
    draft: String,
    /// The file has been read (once, when it is first needed).
    loaded: bool,
}

impl PromptHistory {
    /// Adds a prompt as the newest one. Returns whether the list changed.
    fn remember(&mut self, text: &str) -> bool {
        self.position = None;
        let text = text.trim();
        if text.is_empty() || self.entries.last().is_some_and(|last| last == text) {
            return false;
        }
        self.entries.push(text.to_owned());
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        true
    }

    /// The next older prompt; `current` is kept as the draft when browsing starts.
    fn older(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let at = match self.position {
            None => {
                self.draft = current.to_owned();
                self.entries.len() - 1
            }
            Some(at) => at.saturating_sub(1),
        };
        self.position = Some(at);
        Some(self.entries[at].clone())
    }

    /// The next newer prompt, or the draft once the newest has been passed.
    fn newer(&mut self) -> Option<String> {
        let at = self.position?;
        if at + 1 < self.entries.len() {
            self.position = Some(at + 1);
            Some(self.entries[at + 1].clone())
        } else {
            self.position = None;
            Some(std::mem::take(&mut self.draft))
        }
    }
}

/// Reads the saved prompts: one JSON string per line. Lines that are not are skipped.
fn load(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries = text
        .lines()
        .filter_map(|line| serde_json::from_str::<String>(line).ok())
        .collect::<Vec<_>>();
    let extra = entries.len().saturating_sub(MAX_ENTRIES);
    entries.drain(..extra);
    entries
}

/// Writes the prompts through a temporary file, so a crash never leaves half a history.
fn save(path: &Path, entries: &[String]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = entries
        .iter()
        .filter_map(|entry| serde_json::to_string(entry).ok())
        .collect::<Vec<_>>()
        .join("\n");
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, format!("{text}\n"))?;
    std::fs::rename(&temporary, path)
}

impl App {
    fn history_path(&self) -> Option<PathBuf> {
        self.extensions.dirs.coolcode_join("history.jsonl")
    }

    /// Reads the saved prompts the first time they are needed, and only if sessions are saved.
    fn load_history(&mut self) {
        if self.prompt_history.loaded {
            return;
        }
        self.prompt_history.loaded = true;
        if !self.settings.sessions_enabled {
            return;
        }
        if let Some(path) = self.history_path() {
            let mut saved = load(&path);
            saved.append(&mut self.prompt_history.entries);
            self.prompt_history.entries = saved;
        }
    }

    /// Remembers a prompt that was just sent.
    pub(in crate::tui) fn remember_prompt(&mut self, text: &str) {
        // A redaction value is a secret by definition.
        if text.trim_start().starts_with("/privacy add") {
            return;
        }
        self.load_history();
        if self.prompt_history.remember(text)
            && self.settings.sessions_enabled
            && let Some(path) = self.history_path()
        {
            let _ = save(&path, &self.prompt_history.entries);
        }
    }

    /// Whether ↑ and ↓ browse the prompts: in an empty prompt, and while already browsing.
    pub(in crate::tui) fn history_open(&self) -> bool {
        self.input.is_empty() || self.prompt_history.position.is_some()
    }

    pub(in crate::tui) fn history_up(&mut self) {
        self.load_history();
        let current = self.input.clone();
        if let Some(text) = self.prompt_history.older(&current) {
            self.set_input(text);
        }
    }

    pub(in crate::tui) fn history_down(&mut self) {
        if let Some(text) = self.prompt_history.newer() {
            self.set_input(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::Dirs;
    use crate::tui::state::App;

    fn history(entries: &[&str]) -> PromptHistory {
        PromptHistory {
            entries: entries.iter().map(|entry| (*entry).to_owned()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_and_repeated_prompts_are_not_remembered() {
        let mut history = PromptHistory::default();
        assert!(!history.remember("   "));
        assert!(history.remember("first"));
        assert!(!history.remember("first"), "the same prompt twice in a row");
        assert!(history.remember("second"));
        assert!(history.remember("first"), "but not forgotten for later");
        assert_eq!(history.entries, ["first", "second", "first"]);
    }

    #[test]
    fn only_the_newest_five_hundred_are_kept() {
        let mut history = PromptHistory::default();
        for number in 0..MAX_ENTRIES + 20 {
            history.remember(&format!("prompt {number}"));
        }
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert_eq!(history.entries[0], "prompt 20");
    }

    #[test]
    fn walking_back_and_forth_ends_at_the_draft() {
        let mut history = history(&["a", "b", "c"]);
        assert_eq!(history.older("draft").as_deref(), Some("c"));
        assert_eq!(history.older("ignored").as_deref(), Some("b"));
        assert_eq!(history.older("").as_deref(), Some("a"));
        assert_eq!(
            history.older("").as_deref(),
            Some("a"),
            "stays at the oldest"
        );
        assert_eq!(history.newer().as_deref(), Some("b"));
        assert_eq!(history.newer().as_deref(), Some("c"));
        assert_eq!(
            history.newer().as_deref(),
            Some("draft"),
            "the draft comes back"
        );
        assert_eq!(history.newer(), None, "nothing newer than the draft");
        assert_eq!(PromptHistory::default().older("x"), None);
    }

    #[test]
    fn the_saved_file_round_trips_prompts_with_new_lines_and_skips_bad_lines() {
        let dir = std::env::temp_dir().join(format!("coolcode-history-{}", uuid::Uuid::new_v4()));
        let path = dir.join("history.jsonl");
        let entries = vec!["one".to_owned(), "two\nlines \"quoted\"".to_owned()];
        save(&path, &entries).unwrap();
        assert_eq!(load(&path), entries);
        std::fs::write(&path, "\"kept\"\nnot json\n\"also kept\"\n").unwrap();
        assert_eq!(load(&path), ["kept", "also kept"]);
        assert!(load(&dir.join("missing")).is_empty());
    }

    fn app_in(home: &std::path::Path, save_sessions: bool) -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.settings.sessions_enabled = save_sessions;
        app.extensions.dirs = Dirs::under(home);
        app
    }

    fn home() -> PathBuf {
        std::env::temp_dir().join(format!("coolcode-history-home-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn up_in_an_empty_prompt_recalls_what_was_sent_and_down_brings_the_draft_back() {
        let mut app = app_in(&home(), false);
        app.set_input("/help");
        app.submit().unwrap();
        app.set_input("/usage");
        app.submit().unwrap();
        assert!(app.history_open());
        app.history_up();
        assert_eq!(app.input, "/usage");
        app.history_up();
        assert_eq!(app.input, "/help");
        app.history_down();
        app.history_down();
        assert_eq!(app.input, "", "the empty draft");
        // Text being typed is not replaced by ↑.
        app.set_input("typing");
        assert!(!app.history_open());
    }

    #[test]
    fn prompts_reach_the_disk_only_while_sessions_are_saved() {
        let off = home();
        let mut app = app_in(&off, false);
        app.set_input("/help");
        app.submit().unwrap();
        assert!(
            !off.join(".coolcode/history.jsonl").exists(),
            "nothing is written"
        );
        let on = home();
        let mut app = app_in(&on, true);
        app.set_input("/help");
        app.submit().unwrap();
        assert!(on.join(".coolcode/history.jsonl").is_file());
        // A new run reads it back, when sessions are saved.
        let mut next = app_in(&on, true);
        next.history_up();
        assert_eq!(next.input, "/help");
        // And ignores it when they are not.
        let mut ignoring = app_in(&on, false);
        ignoring.history_up();
        assert_eq!(ignoring.input, "");
    }

    #[test]
    fn a_redaction_value_is_never_remembered() {
        let on = home();
        let mut app = app_in(&on, true);
        app.remember_prompt("/privacy add hunter2");
        app.history_up();
        assert_eq!(app.input, "");
        assert!(!on.join(".coolcode/history.jsonl").exists());
    }
}
