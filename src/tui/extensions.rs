//! Skills, plugins and mods in the interface: the `/` entries they add, running a skill or a
//! plugin command, `/plugin`, the install confirmation and the running mods.

use crate::extensions::Dirs;
use crate::extensions::skills::{Roots, discover};
use crate::tui::slash::{CommandKind, SlashItem};
use crate::tui::state::App;
use anyhow::Result;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long the list of skills and plugin commands for the `/` popup is reused.
const LISTED_FRESH_FOR: Duration = Duration::from_secs(10);
/// Longest description shown in the `/` popup.
const POPUP_DESCRIPTION_CHARS: usize = 90;

pub(in crate::tui) struct Extensions {
    /// Where skills, plugins and mods are read from.
    pub(in crate::tui) dirs: Dirs,
    /// Skills and plugin commands for the `/` popup, when they were read and whether the
    /// folder was trusted then.
    listed: Option<(Instant, bool, Vec<SlashItem>)>,
}

impl Default for Extensions {
    fn default() -> Self {
        Extensions {
            dirs: Dirs::current(),
            listed: None,
        }
    }
}

impl App {
    fn workspace_root(&self) -> Option<PathBuf> {
        std::env::current_dir()
            .and_then(|path| path.canonicalize())
            .ok()
    }

    /// Where skills load from right now (project skills only in a trusted folder).
    pub(in crate::tui) fn skill_roots(&self) -> Roots {
        let root = self.workspace_root().unwrap_or_else(|| PathBuf::from("."));
        Roots::in_dirs(
            &self.extensions.dirs,
            &self.settings,
            &root,
            self.workspace_trusted,
        )
    }

    /// Skills and plugin commands as `/` entries. Names taken by a built-in command are left
    /// out, so a skill can never replace one.
    pub(in crate::tui) fn extension_items(&mut self) -> Vec<SlashItem> {
        if let Some((read_at, trusted, items)) = &self.extensions.listed
            && read_at.elapsed() < LISTED_FRESH_FOR
            && *trusted == self.workspace_trusted
        {
            return items.clone();
        }
        let (skills, _) = discover(&self.skill_roots());
        let items = skills
            .into_iter()
            .filter(|skill| !built_in(&skill.name))
            .map(|skill| SlashItem {
                name: skill.name,
                usage: "[text]".to_owned(),
                summary: shorten(&skill.description, POPUP_DESCRIPTION_CHARS),
                kind: CommandKind::Skill,
            })
            .collect::<Vec<_>>();
        self.extensions.listed = Some((Instant::now(), self.workspace_trusted, items.clone()));
        items
    }

    /// Runs `/name extra text` when `name` is a skill or a plugin command. Returns false when
    /// it is neither, so the caller can report an unknown command.
    pub(in crate::tui) fn run_extension_command(&mut self, value: &str) -> Result<bool> {
        let command = value.strip_prefix('/').unwrap_or(value);
        let (name, extra) = match command.split_once(char::is_whitespace) {
            Some((name, extra)) => (name, extra.trim()),
            None => (command, ""),
        };
        if !crate::extensions::valid_name(name) || built_in(name) {
            return Ok(false);
        }
        let (skills, _) = discover(&self.skill_roots());
        let message = if skills.iter().any(|skill| skill.name == name) {
            if !self.workspace_trusted {
                self.notice =
                    format!("Skills need the workspace tools: trust this folder to run /{name}.");
                return Ok(true);
            }
            skill_message(name, extra)
        } else {
            return Ok(false);
        };
        if self.pending.is_some() {
            self.set_input(value);
            self.notice = "Waiting for the current model response.".to_owned();
            return Ok(true);
        }
        self.send_prompt(message, &std::collections::HashSet::new())?;
        Ok(true)
    }
}

/// Whether `name` is (the first word of) a built-in command.
fn built_in(name: &str) -> bool {
    name == "exit"
        || crate::tui::slash::COMMANDS
            .iter()
            .any(|command| command.name.split_whitespace().next() == Some(name))
}

/// What is sent when the user runs a skill: a request to load it, then their own words.
fn skill_message(name: &str, extra: &str) -> String {
    let mut message =
        format!("Use the skill \"{name}\": load its instructions with use_skill and follow them.");
    if !extra.is_empty() {
        message.push_str("\n\n");
        message.push_str(extra);
    }
    message
}

/// `text` on one line, cut to `limit` characters.
fn shorten(text: &str, limit: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= limit {
        return line;
    }
    line.chars().take(limit - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn home() -> PathBuf {
        let path = std::env::temp_dir().join(format!("coolcode-ext-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("home");
        path
    }

    fn add_skill(home: &std::path::Path, name: &str, description: &str) {
        let folder = home.join(".coolcode/skills").join(name);
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(
            folder.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nSteps.\n"),
        )
        .expect("skill");
    }

    fn app_in(home: &std::path::Path) -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = true;
        app.extensions.dirs = Dirs::under(home);
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            crate::tui::handle_key(
                app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            )
            .expect("type");
        }
    }

    #[test]
    fn skills_are_offered_in_the_popup_and_listed_by_help() {
        let home = home();
        add_skill(&home, "ship", "Ship a release");
        let mut app = app_in(&home);
        type_text(&mut app, "/sh");
        let skill = app
            .slash
            .items
            .iter()
            .find(|item| item.name == "ship")
            .expect("listed");
        assert_eq!(skill.kind, CommandKind::Skill);
        assert_eq!(skill.summary, "Ship a release");
        app.set_input("/help");
        app.submit().expect("help");
        let help = &app.transcript.last().expect("help").text;
        assert!(help.contains("Skills") && help.contains("/ship"), "{help}");
    }

    #[test]
    fn running_a_skill_asks_the_model_to_load_it_and_changes_nothing_else() {
        let home = home();
        add_skill(&home, "ship", "Ship a release");
        let mut app = app_in(&home);
        let mode = app.settings.permission_mode.clone();
        app.set_input("/ship carefully, please");
        app.submit().expect("run");
        let sent = app.messages.last().expect("a message was sent");
        assert!(sent.display.contains("use_skill"), "{}", sent.display);
        assert!(sent.display.contains("\"ship\""), "{}", sent.display);
        assert!(
            sent.display.ends_with("carefully, please"),
            "{}",
            sent.display
        );
        assert!(app.pending.is_some());
        assert_eq!(app.settings.permission_mode, mode);
        let users = app
            .transcript
            .iter()
            .filter(|entry| entry.kind == crate::tui::state::TranscriptKind::User)
            .count();
        assert_eq!(
            users, 1,
            "the command is not shown twice: {:?}",
            app.transcript
        );
    }

    #[test]
    fn a_skill_waits_for_a_trusted_folder_and_for_the_running_turn() {
        let home = home();
        add_skill(&home, "ship", "Ship a release");
        let mut untrusted = app_in(&home);
        untrusted.workspace_trusted = false;
        untrusted.set_input("/ship");
        untrusted.submit().expect("run");
        assert!(untrusted.messages.is_empty());
        assert!(untrusted.notice.contains("trust"), "{}", untrusted.notice);
        let mut busy = app_in(&home);
        let (_sender, receiver) = std::sync::mpsc::channel();
        busy.pending = Some(receiver);
        busy.set_input("/ship now");
        busy.submit().expect("run");
        assert!(busy.messages.is_empty());
        assert_eq!(busy.input, "/ship now", "the text is kept");
        assert!(busy.notice.contains("Waiting"), "{}", busy.notice);
    }

    #[test]
    fn a_skill_cannot_take_the_name_of_a_built_in_command() {
        let home = home();
        add_skill(&home, "help", "Pretends to be help");
        add_skill(&home, "git", "Pretends to be git");
        let mut app = app_in(&home);
        assert!(
            app.extension_items()
                .iter()
                .all(|item| item.name != "help" && item.name != "git"),
            "{:?}",
            app.extension_items()
        );
        app.set_input("/help");
        app.submit().expect("help");
        assert!(app.messages.is_empty(), "the built-in ran");
    }

    #[test]
    fn an_unknown_command_also_considers_skills() {
        let home = home();
        add_skill(&home, "deploy", "Deploy");
        let mut app = app_in(&home);
        app.set_input("/deplyo");
        app.submit().expect("unknown");
        assert!(app.notice.contains("/deploy"), "{}", app.notice);
    }
}
