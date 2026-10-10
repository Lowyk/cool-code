//! Skills, plugins and mods in the interface: the `/` entries they add, running a skill or a
//! plugin command, `/plugin`, the install confirmation and the running mods.

use crate::agent::PendingEvent;
use crate::extensions::Dirs;
use crate::extensions::mods::{self, Event};
use crate::extensions::plugins::{self, PluginCommand};
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
    /// A `/plugin install` in progress.
    pub(in crate::tui) install: Option<crate::tui::plugin_install::Install>,
    /// The running mods.
    pub(in crate::tui) mods: crate::extensions::mod_host::ModHost,
    /// Mods run only once the interface itself is running, never in an `App` built for a test.
    pub(in crate::tui) mods_active: bool,
}

impl Default for Extensions {
    fn default() -> Self {
        Extensions {
            dirs: Dirs::current(),
            listed: None,
            install: None,
            mods: Default::default(),
            mods_active: false,
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
    /// out, so neither can ever replace one, and a skill comes before a plugin command.
    pub(in crate::tui) fn extension_items(&mut self) -> Vec<SlashItem> {
        if let Some((read_at, trusted, items)) = &self.extensions.listed
            && read_at.elapsed() < LISTED_FRESH_FOR
            && *trusted == self.workspace_trusted
        {
            return items.clone();
        }
        let (skills, _) = discover(&self.skill_roots());
        let mut items: Vec<SlashItem> = Vec::new();
        for skill in skills {
            if !built_in(&skill.name) {
                items.push(SlashItem {
                    name: skill.name,
                    usage: "[text]".to_owned(),
                    summary: shorten(&skill.description, POPUP_DESCRIPTION_CHARS),
                    kind: CommandKind::Skill,
                });
            }
        }
        for (_, command) in self.plugin_commands() {
            if !built_in(&command.name) && !items.iter().any(|item| item.name == command.name) {
                items.push(SlashItem {
                    name: command.name,
                    usage: "[text]".to_owned(),
                    summary: shorten(&command.description, POPUP_DESCRIPTION_CHARS),
                    kind: CommandKind::Plugin,
                });
            }
        }
        self.extensions.listed = Some((Instant::now(), self.workspace_trusted, items.clone()));
        items
    }

    /// The commands of the enabled plugins, with the plugin each comes from.
    fn plugin_commands(&self) -> Vec<(String, PluginCommand)> {
        let (installed, _) = plugins::installed(&self.extensions.dirs, &self.settings);
        installed
            .into_iter()
            .filter(|plugin| plugin.enabled)
            .flat_map(|plugin| {
                let name = plugin.manifest.name.clone();
                plugin
                    .manifest
                    .commands
                    .into_iter()
                    .map(move |command| (name.clone(), command))
            })
            .collect()
    }

    /// Forgets the cached `/` entries, after something was installed, removed or switched.
    pub(in crate::tui) fn forget_extension_items(&mut self) {
        self.extensions.listed = None;
    }

    /// Picks up finished plugin fetches and what the mods sent.
    pub(in crate::tui) fn poll_extensions(&mut self) {
        self.poll_plugin_install();
        let notices = self.extensions.mods.poll();
        if !notices.is_empty() {
            let start = notices.len().saturating_sub(3);
            self.notice = notices[start..].join("  ·  ");
        }
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
        } else if let Some((_, command)) = self
            .plugin_commands()
            .into_iter()
            .find(|(_, command)| command.name == name)
        {
            command.expand(extra)
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

    /// Starts the approved mods. Called once the interface is running.
    pub(in crate::tui) fn start_mods(&mut self) {
        self.extensions.mods_active = true;
        self.sync_mods();
    }

    /// Stops every mod, when the program exits.
    pub(in crate::tui) fn stop_mods(&mut self) {
        self.extensions.mods_active = false;
        self.extensions.mods.stop_all();
    }

    /// Makes the running mods match the approved, switched-on ones: stops the others, restarts
    /// one whose manifest changed, and starts the new ones.
    pub(in crate::tui) fn sync_mods(&mut self) {
        if !self.extensions.mods_active {
            return;
        }
        let (found, _) = mods::discover(&self.extensions.dirs, &self.settings);
        let wanted = found
            .into_iter()
            .filter(|info| mods::should_run(&self.settings, info))
            .collect::<Vec<_>>();
        let host = &mut self.extensions.mods;
        for id in host.running_ids() {
            let still = wanted
                .iter()
                .find(|info| info.id == id)
                .is_some_and(|info| host.running_info(&id) == Some(info));
            if !still {
                host.stop(&id);
            }
        }
        let started = self.session_event();
        let host = &mut self.extensions.mods;
        let running = host.running_ids();
        for info in wanted {
            if !running.contains(&info.id) {
                host.start(info, Some(&started));
            }
        }
    }

    fn session_event(&self) -> Event {
        Event::SessionStarted {
            project: self
                .workspace_root()
                .and_then(|root| {
                    root.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_default(),
            mode: self.settings.permission_mode.clone(),
            model: self.settings.model.clone().unwrap_or_default(),
        }
    }

    /// Tells the mods that something happened.
    pub(in crate::tui) fn mod_event(&mut self, event: Event) {
        self.extensions.mods.send(&event);
    }

    /// Tells the mods about a tool or the end of a turn reported by the worker.
    pub(in crate::tui) fn observe_for_mods(
        &mut self,
        event: &std::result::Result<PendingEvent, std::sync::mpsc::TryRecvError>,
    ) {
        let event = match event {
            Ok(PendingEvent::ToolStarted(label)) => Event::ToolStarted {
                tool: tool_name(label),
            },
            Ok(PendingEvent::ToolAction(action)) => {
                // "Tool · name · summary", as the agent loop writes it.
                let mut parts = action.splitn(3, " · ").skip(1);
                let tool = tool_name(parts.next().unwrap_or_default());
                let summary = parts.next().unwrap_or_default();
                Event::ToolFinished {
                    tool,
                    ok: !summary.starts_with("Tool error"),
                }
            }
            Ok(PendingEvent::Finished(Ok(_))) => Event::TurnFinished { outcome: "done" },
            Ok(PendingEvent::Finished(Err(_))) => Event::TurnFinished { outcome: "failed" },
            _ => return,
        };
        self.mod_event(event);
    }
}

/// A tool's name for a mod. A running command is labelled with its command line, which is
/// never passed on: anything that is not a tool name counts as `run_command`.
fn tool_name(label: &str) -> String {
    let known = crate::tools::definitions()
        .iter()
        .map(|tool| tool.name)
        .chain(["spawn_subagents", "generate_image", "use_skill"])
        .any(|name| name == label);
    if known {
        label.to_owned()
    } else {
        "run_command".to_owned()
    }
}

/// Whether `name` is (the first word of) a built-in command.
pub(in crate::tui) fn built_in(name: &str) -> bool {
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

    fn status_row(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).expect("terminal");
        terminal
            .draw(|frame| crate::tui::render::draw(frame, app, 0))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol().to_owned())
                    .collect::<String>()
            })
            .find(|row| row.contains("no model selected"))
            .expect("the status line")
    }

    fn poll_until(app: &mut App, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(app) {
            assert!(
                Instant::now() < deadline,
                "timed out; notice: {}",
                app.notice
            );
            app.poll_response();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_running_mod_shows_its_status_and_hears_about_the_turn() {
        let mut app = app_in(&home());
        let info = crate::extensions::mod_host::tests::helper("echo", &["turn_finished"]);
        app.extensions.mods.start(info, None);
        poll_until(&mut app, |app| app.extensions.mods.statuses() == ["ready"]);
        assert!(status_row(&app).contains("ready"), "{}", status_row(&app));
        let (sender, receiver) = std::sync::mpsc::channel();
        app.pending = Some(receiver);
        app.streaming = Some(crate::tui::state::StreamingTurn::new(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        sender
            .send(PendingEvent::Finished(Ok(crate::provider::Completion {
                text: "Done.".to_owned(),
                provider_id: None,
                model_id: "m".to_owned(),
                failed_over: false,
                tool_calls: Vec::new(),
            })))
            .unwrap();
        poll_until(&mut app, |app| {
            app.extensions.mods.statuses() == ["tick line"]
        });
        assert!(status_row(&app).contains("tick line"));
        poll_until(&mut app, |app| {
            app.notice.contains("helper-echo: turn [31mdone")
        });
        app.stop_mods();
        assert!(app.extensions.mods.running_ids().is_empty());
    }

    #[test]
    fn only_approved_and_switched_on_mods_are_started() {
        let home = home();
        let helper = crate::extensions::mod_host::tests::helper("echo", &[]);
        let folder = home.join(".coolcode/mods/watcher");
        std::fs::create_dir_all(&folder).unwrap();
        let manifest = crate::extensions::mods::ModManifest {
            name: "watcher".to_owned(),
            ..helper.manifest
        };
        std::fs::write(folder.join("mod.toml"), toml::to_string(&manifest).unwrap()).unwrap();
        let mut app = app_in(&home);
        app.sync_mods();
        assert!(
            app.extensions.mods.running_ids().is_empty(),
            "not before the app runs"
        );
        app.start_mods();
        assert!(app.extensions.mods.running_ids().is_empty(), "not approved");
        let (found, _) = mods::discover(&app.extensions.dirs, &app.settings);
        mods::approve(&mut app.settings, &found[0]);
        app.sync_mods();
        assert_eq!(app.extensions.mods.running_ids(), ["watcher"]);
        poll_until(&mut app, |app| app.extensions.mods.statuses() == ["ready"]);
        app.settings.disabled_mods.push("watcher".to_owned());
        app.sync_mods();
        assert!(app.extensions.mods.running_ids().is_empty(), "switched off");
        app.settings.disabled_mods.clear();
        std::fs::write(
            folder.join("mod.toml"),
            toml::to_string(&crate::extensions::mods::ModManifest {
                events: vec!["turn_finished".to_owned()],
                ..manifest
            })
            .unwrap(),
        )
        .unwrap();
        app.sync_mods();
        assert!(
            app.extensions.mods.running_ids().is_empty(),
            "a changed manifest needs approval again"
        );
        app.stop_mods();
    }

    #[test]
    fn tool_events_name_the_tool_but_never_the_command_line() {
        assert_eq!(tool_name("read_file"), "read_file");
        assert_eq!(tool_name("use_skill"), "use_skill");
        assert_eq!(
            tool_name("curl -H 'Authorization: secret' x"),
            "run_command"
        );
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
