//! Mods: external programs that watch Cool Code. A mod reads one JSON event per line on its
//! standard input and writes one JSON message per line on its standard output:
//! `{"status": "text"}` sets its part of the status line and `{"toast": "text"}` shows a notice.
//!
//! Mods are observers. Nothing they send can approve, block or change an action. A mod runs
//! only after the user approved its manifest (recorded with a hash, so a changed manifest needs
//! approval again), gets a small environment without the user's variables, and is stopped when
//! the program exits. A mod that crashes, hangs or prints garbage is reported and ignored.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The events a mod can ask for.
pub(crate) const EVENTS: [&str; 5] = [
    "session_started",
    "prompt_submitted",
    "tool_started",
    "tool_finished",
    "turn_finished",
];

/// Longest status a mod can show, in characters.
pub(crate) const MAX_STATUS_CHARS: usize = 40;
/// Longest notice a mod can show, in characters.
pub(crate) const MAX_TOAST_CHARS: usize = 160;
/// Largest `mod.toml`.
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;

/// `mod.toml`, or one `[[mods]]` entry of a plugin.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModManifest {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    /// The program: a name looked up on PATH, an absolute path, or a path inside the mod's
    /// folder such as `./mod.py`.
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    /// Which of [`EVENTS`] it receives.
    #[serde(default)]
    pub(crate) events: Vec<String>,
}

impl ModManifest {
    pub(crate) fn validate(&self) -> Result<()> {
        if !crate::extensions::valid_name(&self.name) {
            bail!(
                "the mod name {:?} is not valid: use lowercase letters, digits, - and _",
                self.name
            );
        }
        if self.command.trim().is_empty() || self.command.contains('\0') {
            bail!("the mod {} has no command to run", self.name);
        }
        if self.description.chars().count() > 300 {
            bail!(
                "the mod {}'s description is longer than 300 characters",
                self.name
            );
        }
        if self.args.len() > 64 {
            bail!("the mod {} has more than 64 arguments", self.name);
        }
        if let Some(index) = self
            .args
            .iter()
            .position(|arg| arg.contains('\0') || arg.len() > 4096)
        {
            bail!(
                "argument {} of the mod {} is not valid",
                index + 1,
                self.name
            );
        }
        for (index, event) in self.events.iter().enumerate() {
            if !EVENTS.contains(&event.as_str()) {
                bail!(
                    "the mod {} asks for an unknown event {event:?}; the events are {}",
                    self.name,
                    EVENTS.join(", ")
                );
            }
            if self.events[..index].contains(event) {
                bail!("the mod {} lists the event {event} twice", self.name);
            }
        }
        Ok(())
    }

    /// The command line exactly as it will run, for showing to the user.
    pub(crate) fn command_line(&self) -> String {
        std::iter::once(&self.command)
            .chain(&self.args)
            .map(|part| {
                if !part.is_empty() && !part.contains(|c: char| c.is_whitespace() || c == '"') {
                    part.clone()
                } else {
                    format!("\"{}\"", part.replace('"', "\\\""))
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A mod found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModInfo {
    /// `name` for a mod in `~/.coolcode/mods`, `plugin/name` for a plugin's mod.
    pub(crate) id: String,
    pub(crate) manifest: ModManifest,
    /// Where it runs, and where a relative command is looked up.
    pub(crate) dir: PathBuf,
}

/// Reads and checks `<dir>/mod.toml`.
pub(crate) fn read_manifest(dir: &Path) -> Result<ModManifest> {
    let path = dir.join("mod.toml");
    let size = std::fs::metadata(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if size > MAX_MANIFEST_BYTES {
        bail!("{} is larger than 16 KiB", path.display());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: ModManifest =
        toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
    manifest.validate()?;
    Ok(manifest)
}

/// Every mod: those in `~/.coolcode/mods/<name>/mod.toml`, then those of enabled plugins, and a
/// warning for each one that could not be read.
pub(crate) fn discover(
    dirs: &crate::extensions::Dirs,
    settings: &crate::Settings,
) -> (Vec<ModInfo>, Vec<String>) {
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    if let Some(Ok(entries)) = dirs.coolcode_join("mods").map(std::fs::read_dir) {
        let mut folders = entries
            .flatten()
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .filter(|entry| entry.path().join("mod.toml").is_file())
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        folders.sort();
        for dir in folders {
            let folder = dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match read_manifest(&dir) {
                Ok(manifest) if manifest.name == folder => found.push(ModInfo {
                    id: manifest.name.clone(),
                    manifest,
                    dir,
                }),
                Ok(manifest) => warnings.push(format!(
                    "the mod in mods/{folder} is named {}; its folder must have the same name",
                    manifest.name
                )),
                Err(error) => warnings.push(format!("the mod in mods/{folder}: {error:#}")),
            }
        }
    }
    let (plugins, _) = crate::extensions::plugins::installed(dirs, settings);
    for plugin in plugins.iter().filter(|plugin| plugin.enabled) {
        found.extend(plugin.mods());
    }
    (found, warnings)
}

/// The fingerprint an approval is recorded with: anything that changes what runs changes it.
pub(crate) fn manifest_hash(info: &ModInfo) -> String {
    use sha2::Digest as _;
    let described = serde_json::json!({
        "id": info.id,
        "dir": info.dir.to_string_lossy(),
        "manifest": info.manifest,
    });
    sha2::Sha256::digest(described.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether the user approved this exact manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Approval {
    Approved,
    NotApproved,
    /// Approved once, but the manifest has changed since.
    Changed,
}

pub(crate) fn approval(settings: &crate::Settings, info: &ModInfo) -> Approval {
    match settings.approved_mods.get(&info.id) {
        None => Approval::NotApproved,
        Some(hash) if *hash == manifest_hash(info) => Approval::Approved,
        Some(_) => Approval::Changed,
    }
}

/// Approved and switched on.
pub(crate) fn should_run(settings: &crate::Settings, info: &ModInfo) -> bool {
    approval(settings, info) == Approval::Approved && !settings.disabled_mods.contains(&info.id)
}

/// Records the user's approval of the manifest as it is now, and switches the mod on.
pub(crate) fn approve(settings: &mut crate::Settings, info: &ModInfo) {
    settings
        .approved_mods
        .insert(info.id.clone(), manifest_hash(info));
    settings.disabled_mods.retain(|id| *id != info.id);
}

/// What a mod asked for in one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModMessage {
    /// Its status-line text; empty clears it.
    Status(String),
    Toast(String),
}

/// Reads one line from a mod. `Err` holds why the line was not understood.
pub(crate) fn parse_line(line: &str) -> std::result::Result<Vec<ModMessage>, String> {
    use serde_json::Value;
    let line = line.trim();
    if line.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(line).map_err(|_| "it is not JSON".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "it is not a JSON object".to_owned())?;
    let mut messages = Vec::new();
    for (key, value) in object {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Null if key == "status" => String::new(),
            _ => return Err(format!("\"{key}\" must be text")),
        };
        messages.push(match key.as_str() {
            "status" => ModMessage::Status(text),
            "toast" => ModMessage::Toast(text),
            other => return Err(format!("\"{other}\" is not something a mod can send")),
        });
    }
    if messages.is_empty() {
        return Err("it has neither \"status\" nor \"toast\"".to_owned());
    }
    Ok(messages)
}

/// `text` made safe to draw: control characters (escape sequences, line breaks, tabs) and
/// direction overrides removed, spaces collapsed, cut to `max_chars`.
pub(crate) fn sanitize(text: &str, max_chars: usize) -> String {
    let cleaned = text
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some(' ')
            } else if c.is_control()
                || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                None
            } else {
                Some(c)
            }
        })
        .collect::<String>();
    let line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max_chars {
        return line;
    }
    line.chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        + "…"
}

/// Something that happened, as sent to mods. Only small facts: never the text of a prompt, a
/// command line, a file or a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    SessionStarted {
        project: String,
        mode: String,
        model: String,
    },
    PromptSubmitted {
        chars: usize,
    },
    ToolStarted {
        tool: String,
    },
    ToolFinished {
        tool: String,
        ok: bool,
    },
    TurnFinished {
        outcome: &'static str,
    },
}

impl Event {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Event::SessionStarted { .. } => "session_started",
            Event::PromptSubmitted { .. } => "prompt_submitted",
            Event::ToolStarted { .. } => "tool_started",
            Event::ToolFinished { .. } => "tool_finished",
            Event::TurnFinished { .. } => "turn_finished",
        }
    }

    /// The event as one JSON line.
    pub(crate) fn to_line(&self) -> String {
        let mut value = match self {
            Event::SessionStarted {
                project,
                mode,
                model,
            } => serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "project": project,
                "mode": mode,
                "model": model,
            }),
            Event::PromptSubmitted { chars } => serde_json::json!({ "chars": chars }),
            Event::ToolStarted { tool } => serde_json::json!({ "tool": tool }),
            Event::ToolFinished { tool, ok } => serde_json::json!({ "tool": tool, "ok": ok }),
            Event::TurnFinished { outcome } => serde_json::json!({ "outcome": outcome }),
        };
        value["event"] = serde_json::Value::from(self.name());
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(command: &str, args: &[&str], events: &[&str]) -> ModManifest {
        ModManifest {
            name: "clock".to_owned(),
            description: "Shows the time".to_owned(),
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            events: events.iter().map(|event| (*event).to_owned()).collect(),
        }
    }

    fn info(manifest: ModManifest) -> ModInfo {
        ModInfo {
            id: "clock".to_owned(),
            manifest,
            dir: PathBuf::from("/mods/clock"),
        }
    }

    #[test]
    fn a_manifest_is_checked_before_anything_runs() {
        assert!(
            manifest("python3", &["mod.py"], &["turn_finished"])
                .validate()
                .is_ok()
        );
        for (bad, reason) in [
            (manifest("", &[], &[]), "command"),
            (manifest("python3", &[], &["everything"]), "everything"),
            (manifest("python3", &["a\0b"], &[]), "argument"),
            (
                manifest("python3", &[], &["turn_finished", "turn_finished"]),
                "twice",
            ),
            (
                ModManifest {
                    name: "../up".to_owned(),
                    ..manifest("x", &[], &[])
                },
                "name",
            ),
        ] {
            let error = bad.validate().expect_err(reason);
            assert!(format!("{error:#}").contains(reason), "{error:#}");
        }
        let too_many = manifest("x", &["a"; 65], &[]);
        assert!(too_many.validate().is_err());
    }

    #[test]
    fn the_command_line_is_shown_exactly_with_quoting() {
        assert_eq!(
            manifest("python3", &["mod.py", "--every", "5 s"], &[]).command_line(),
            "python3 mod.py --every \"5 s\""
        );
        assert_eq!(manifest("./run", &[], &[]).command_line(), "./run");
        assert_eq!(
            manifest("node", &["say \"hi\""], &[]).command_line(),
            "node \"say \\\"hi\\\"\""
        );
    }

    #[test]
    fn mod_toml_is_read_and_unknown_keys_are_refused() {
        let dir = std::env::temp_dir().join(format!("coolcode-mod-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mod.toml"),
            "name = \"clock\"\ncommand = \"python3\"\nargs = [\"clock.py\"]\nevents = [\"turn_finished\"]\n",
        )
        .unwrap();
        let read = read_manifest(&dir).expect("read");
        assert_eq!(
            read,
            manifest("python3", &["clock.py"], &["turn_finished"]).with_description("")
        );
        std::fs::write(
            dir.join("mod.toml"),
            "name = \"clock\"\ncommand = \"python3\"\napprove = true\n",
        )
        .unwrap();
        assert!(read_manifest(&dir).is_err(), "no hidden extras");
        assert!(read_manifest(&dir.join("missing")).is_err());
    }

    impl ModManifest {
        fn with_description(mut self, description: &str) -> ModManifest {
            self.description = description.to_owned();
            self
        }
    }

    #[test]
    fn an_approval_holds_only_for_the_manifest_that_was_approved() {
        let original = info(manifest("python3", &["clock.py"], &["turn_finished"]));
        let mut settings = crate::Settings::default();
        assert_eq!(approval(&settings, &original), Approval::NotApproved);
        assert!(!should_run(&settings, &original));
        approve(&mut settings, &original);
        assert_eq!(approval(&settings, &original), Approval::Approved);
        assert!(should_run(&settings, &original));
        for changed in [
            info(manifest("python3", &["other.py"], &["turn_finished"])),
            info(manifest("bash", &["clock.py"], &["turn_finished"])),
            info(manifest("python3", &["clock.py"], &["tool_started"])),
            ModInfo {
                dir: PathBuf::from("/elsewhere"),
                ..original.clone()
            },
        ] {
            assert_ne!(manifest_hash(&changed), manifest_hash(&original));
            assert_eq!(approval(&settings, &changed), Approval::Changed);
            assert!(!should_run(&settings, &changed), "{changed:?}");
        }
        settings.disabled_mods.push("clock".to_owned());
        assert!(!should_run(&settings, &original), "switched off");
        assert_eq!(manifest_hash(&original), manifest_hash(&original.clone()));
        assert_eq!(manifest_hash(&original).len(), 64);
    }

    #[test]
    fn mods_are_found_in_their_folder_and_in_enabled_plugins() {
        let home = std::env::temp_dir().join(format!("coolcode-mods-{}", uuid::Uuid::new_v4()));
        let dirs = crate::extensions::Dirs::under(&home);
        let mods = home.join(".coolcode/mods");
        for (folder, text) in [
            ("clock", "name = \"clock\"\ncommand = \"python3\"\n"),
            ("broken", "name = \"broken\"\n"),
            ("renamed", "name = \"other\"\ncommand = \"x\"\n"),
        ] {
            std::fs::create_dir_all(mods.join(folder)).unwrap();
            std::fs::write(mods.join(folder).join("mod.toml"), text).unwrap();
        }
        let plugin = home.join(".coolcode/plugins/kit");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.toml"),
            "name = \"kit\"\nversion = \"1\"\ndescription = \"Kit\"\n[[mods]]\nname = \"watch\"\ncommand = \"node\"\n",
        )
        .unwrap();
        let mut settings = crate::Settings::default();
        let (found, warnings) = discover(&dirs, &settings);
        let ids = found
            .iter()
            .map(|info| info.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["clock", "kit/watch"]);
        assert_eq!(found[0].dir, mods.join("clock"));
        assert_eq!(found[1].dir, plugin);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        settings.disabled_plugins.push("kit".to_owned());
        let (found, _) = discover(&dirs, &settings);
        assert_eq!(found.len(), 1, "a disabled plugin's mods are not offered");
    }

    #[test]
    fn messages_are_status_or_toast_and_anything_else_is_refused() {
        assert_eq!(
            parse_line(r#"{"status": "3 tests failing"}"#),
            Ok(vec![ModMessage::Status("3 tests failing".to_owned())])
        );
        assert_eq!(
            parse_line(r#"{"toast": "Build finished"}"#),
            Ok(vec![ModMessage::Toast("Build finished".to_owned())])
        );
        assert_eq!(
            parse_line(r#"{"status": "a", "toast": "b"}"#),
            Ok(vec![
                ModMessage::Status("a".to_owned()),
                ModMessage::Toast("b".to_owned())
            ])
        );
        assert_eq!(
            parse_line(r#"{"status": null}"#),
            Ok(vec![ModMessage::Status(String::new())]),
            "null clears the status"
        );
        assert_eq!(parse_line("   "), Ok(Vec::new()), "blank lines are fine");
        for garbage in [
            "hello",
            "[1, 2]",
            r#"{"approve": true}"#,
            r#"{"status": 5}"#,
            r#"{"toast": {"text": "x"}}"#,
            "{\"status\": \"unterminated",
        ] {
            assert!(parse_line(garbage).is_err(), "{garbage}");
        }
    }

    #[test]
    fn mod_text_is_cleaned_of_control_characters_and_cut() {
        assert_eq!(sanitize("\u{1b}[31mred\u{1b}[0m", 40), "[31mred[0m");
        assert_eq!(
            sanitize("two\nlines\tand\r\u{7}bell", 40),
            "two lines and bell"
        );
        assert_eq!(sanitize("  spaced    out  ", 40), "spaced out");
        assert_eq!(
            sanitize("abc\u{202e}def\u{2066}", 40),
            "abcdef",
            "no direction tricks"
        );
        assert_eq!(sanitize("\u{9b}31m", 40), "31m", "C1 controls too");
        assert_eq!(
            sanitize(&"x".repeat(100), 10),
            format!("{}…", "x".repeat(9))
        );
        assert_eq!(sanitize("ёжик", 3), "ёж…");
    }

    #[test]
    fn events_are_single_small_json_lines() {
        let events = [
            Event::SessionStarted {
                project: "app".to_owned(),
                mode: "plan".to_owned(),
                model: "m".to_owned(),
            },
            Event::PromptSubmitted { chars: 12 },
            Event::ToolStarted {
                tool: "read_file".to_owned(),
            },
            Event::ToolFinished {
                tool: "read_file".to_owned(),
                ok: true,
            },
            Event::TurnFinished { outcome: "done" },
        ];
        for event in events {
            let line = event.to_line();
            assert!(!line.contains('\n') && line.len() < 512, "{line}");
            let value: serde_json::Value = serde_json::from_str(&line).expect("json");
            assert_eq!(value["event"], event.name());
            assert!(EVENTS.contains(&event.name()));
        }
        let prompt: serde_json::Value =
            serde_json::from_str(&Event::PromptSubmitted { chars: 12 }.to_line()).unwrap();
        assert_eq!(prompt["chars"], 12);
    }
}
