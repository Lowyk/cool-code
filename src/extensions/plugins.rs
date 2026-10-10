//! Plugins: a folder or Git repository with a `plugin.toml` that names the plugin and lists
//! what it adds: folders of skills, slash commands that expand to a prompt, and mods.
//!
//! Installing copies or clones it into `~/.coolcode/plugins/<name>` through a staging folder:
//! the user is shown exactly what it adds, every mod's command line included, and nothing is
//! kept (or run) unless they confirm. Names are checked, nothing is overwritten, and every path
//! a plugin declares has to stay inside its own folder.

use crate::extensions::Dirs;
use crate::extensions::mods::{ModInfo, ModManifest};
use crate::extensions::skills::Skill;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Largest `plugin.toml`.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Most files and bytes copied from a local folder.
const MAX_COPY_FILES: usize = 2_000;
const MAX_COPY_BYTES: u64 = 50 * 1024 * 1024;
/// How long a `git clone` may take.
const CLONE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// `plugin.toml`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) description: String,
    /// Folders, relative to the plugin, that hold skill folders.
    #[serde(default)]
    pub(crate) skills: Vec<String>,
    #[serde(default)]
    pub(crate) commands: Vec<PluginCommand>,
    #[serde(default)]
    pub(crate) mods: Vec<ModManifest>,
}

/// A slash command that expands to a prompt.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginCommand {
    pub(crate) name: String,
    pub(crate) description: String,
    /// The message sent; `$ARGUMENTS` is replaced by what the user typed after the command.
    pub(crate) prompt: String,
}

impl PluginCommand {
    /// The message for `/name extra`.
    pub(crate) fn expand(&self, extra: &str) -> String {
        let extra = extra.trim();
        if self.prompt.contains("$ARGUMENTS") {
            return self.prompt.replace("$ARGUMENTS", extra);
        }
        if extra.is_empty() {
            self.prompt.clone()
        } else {
            format!("{}\n\n{extra}", self.prompt)
        }
    }
}

/// A path inside the plugin, written relative to it, with no `..`.
fn plain_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path.components().all(|part| {
            matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

impl Manifest {
    /// Checks the manifest of the plugin in `dir`.
    pub(crate) fn validate(&self, dir: &Path) -> Result<()> {
        if !crate::extensions::valid_name(&self.name) {
            bail!(
                "the plugin name {:?} is not valid: use lowercase letters, digits, - and _",
                self.name
            );
        }
        let version = self.version.trim();
        if version.is_empty()
            || version.chars().count() > 40
            || version.contains(|c: char| c.is_whitespace() || c.is_control())
        {
            bail!("the plugin needs a short version such as 1.0.0");
        }
        if self.description.trim().is_empty() || self.description.chars().count() > 300 {
            bail!("the plugin needs a description of at most 300 characters");
        }
        if self.skills.len() > 16 || self.commands.len() > 50 || self.mods.len() > 16 {
            bail!("a plugin can list at most 16 skill folders, 50 commands and 16 mods");
        }
        let root = dir
            .canonicalize()
            .with_context(|| format!("reading {}", dir.display()))?;
        for folder in &self.skills {
            if !plain_relative(folder) {
                bail!("the skill folder {folder:?} must be a path inside the plugin");
            }
            let real = root
                .join(folder)
                .canonicalize()
                .with_context(|| format!("the skill folder {folder} is missing"))?;
            if !real.starts_with(&root) || !real.is_dir() {
                bail!("the skill folder {folder:?} must be a folder inside the plugin");
            }
        }
        for (index, command) in self.commands.iter().enumerate() {
            if !crate::extensions::valid_name(&command.name) {
                bail!(
                    "the command name {:?} is not valid: use lowercase letters, digits, - and _",
                    command.name
                );
            }
            if self.commands[..index]
                .iter()
                .any(|earlier| earlier.name == command.name)
            {
                bail!("the command {} is listed twice", command.name);
            }
            if command.description.trim().is_empty() || command.description.chars().count() > 300 {
                bail!(
                    "the command {} needs a description of at most 300 characters",
                    command.name
                );
            }
            if command.prompt.trim().is_empty() || command.prompt.len() > 16 * 1024 {
                bail!(
                    "the command {} needs a prompt of at most 16 KiB",
                    command.name
                );
            }
        }
        for (index, entry) in self.mods.iter().enumerate() {
            entry.validate()?;
            if self.mods[..index]
                .iter()
                .any(|earlier| earlier.name == entry.name)
            {
                bail!("the mod {} is listed twice", entry.name);
            }
        }
        Ok(())
    }
}

/// Reads and checks `<dir>/plugin.toml`.
pub(crate) fn read_manifest(dir: &Path) -> Result<Manifest> {
    let path = dir.join("plugin.toml");
    let size = std::fs::metadata(&path)
        .with_context(|| format!("there is no plugin.toml in {}", dir.display()))?
        .len();
    if size > MAX_MANIFEST_BYTES {
        bail!("plugin.toml is larger than 64 KiB");
    }
    let text = std::fs::read_to_string(&path).context("reading plugin.toml")?;
    let manifest: Manifest = toml::from_str(&text).context("reading plugin.toml")?;
    manifest.validate(dir)?;
    Ok(manifest)
}

/// An installed plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plugin {
    pub(crate) manifest: Manifest,
    pub(crate) dir: PathBuf,
    pub(crate) enabled: bool,
}

impl Plugin {
    /// Its skill folders, resolved; each is inside the plugin's folder.
    pub(crate) fn skill_folders(&self) -> Vec<PathBuf> {
        let Ok(root) = self.dir.canonicalize() else {
            return Vec::new();
        };
        self.manifest
            .skills
            .iter()
            .filter_map(|folder| root.join(folder).canonicalize().ok())
            .filter(|real| real.starts_with(&root))
            .collect()
    }

    /// Its mods, with ids of the form `plugin/mod`.
    pub(crate) fn mods(&self) -> Vec<ModInfo> {
        mods_in(&self.manifest, &self.dir)
    }
}

fn mods_in(manifest: &Manifest, dir: &Path) -> Vec<ModInfo> {
    manifest
        .mods
        .iter()
        .map(|entry| ModInfo {
            id: format!("{}/{}", manifest.name, entry.name),
            manifest: entry.clone(),
            dir: dir.to_path_buf(),
        })
        .collect()
}

/// `~/.coolcode/plugins`.
pub(crate) fn plugins_dir(dirs: &Dirs) -> Option<PathBuf> {
    dirs.coolcode_join("plugins")
}

/// Every installed plugin, sorted by name, and a warning for each folder that is not a valid
/// plugin.
pub(crate) fn installed(dirs: &Dirs, settings: &crate::Settings) -> (Vec<Plugin>, Vec<String>) {
    let mut plugins = Vec::new();
    let mut warnings = Vec::new();
    let Some(Ok(entries)) = plugins_dir(dirs).map(std::fs::read_dir) else {
        return (plugins, warnings);
    };
    let mut folders = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    folders.sort();
    for dir in folders {
        let folder = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match read_manifest(&dir) {
            Ok(manifest) if manifest.name != folder => warnings.push(format!(
                "the plugin in plugins/{folder} calls itself {}; reinstall it",
                manifest.name
            )),
            Ok(manifest) => plugins.push(Plugin {
                enabled: !settings.disabled_plugins.contains(&manifest.name),
                manifest,
                dir,
            }),
            Err(error) => warnings.push(format!("the plugin in plugins/{folder}: {error:#}")),
        }
    }
    (plugins, warnings)
}

/// Where a plugin is installed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Git(String),
    Local(PathBuf),
}

/// Reads what the user typed after `/plugin install`.
pub(crate) fn parse_source(text: &str) -> Result<Source> {
    let text = text.trim();
    if text.is_empty() {
        bail!("say where the plugin is: a Git address or a folder");
    }
    if text.starts_with('-') {
        bail!("{text:?} is not a Git address or a folder");
    }
    let git = ["https://", "http://", "ssh://", "git://", "file://", "git@"]
        .iter()
        .any(|prefix| text.starts_with(prefix));
    if git {
        return Ok(Source::Git(text.to_owned()));
    }
    let path = match text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        Some(rest) => dirs::home_dir()
            .context("could not find the home folder")?
            .join(rest),
        None => PathBuf::from(text),
    };
    let real = path
        .canonicalize()
        .with_context(|| format!("there is no folder at {text}"))?;
    if !real.is_dir() {
        bail!("{text} is not a folder");
    }
    Ok(Source::Local(real))
}

/// A plugin fetched into a staging folder, waiting for the user's confirmation.
#[derive(Debug)]
pub(crate) struct Staged {
    pub(crate) manifest: Manifest,
    /// Where it was fetched to.
    pub(crate) staging: PathBuf,
    /// Where it goes when confirmed.
    pub(crate) target: PathBuf,
    /// What the user typed.
    pub(crate) source: String,
    pub(crate) skills: Vec<Skill>,
}

/// Fetches a plugin into a staging folder under `~/.coolcode/plugins` and checks it. Nothing
/// from it runs here.
pub(crate) fn stage(dirs: &Dirs, source: &Source) -> Result<Staged> {
    let plugins = plugins_dir(dirs).context("could not find the home folder")?;
    std::fs::create_dir_all(&plugins).with_context(|| format!("creating {}", plugins.display()))?;
    let plugins = plugins
        .canonicalize()
        .context("reading the plugins folder")?;
    let staging = plugins.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    let checked = fetch(source, &staging).and_then(|()| {
        let manifest = read_manifest(&staging)?;
        let target = plugins.join(&manifest.name);
        if std::fs::symlink_metadata(&target).is_ok() {
            bail!(
                "a plugin named {} is already installed; remove it first with /plugin remove {}",
                manifest.name,
                manifest.name
            );
        }
        Ok((manifest, target))
    });
    let (manifest, target) = match checked {
        Ok(checked) => checked,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let plugin = Plugin {
        manifest: manifest.clone(),
        dir: staging.clone(),
        enabled: true,
    };
    let roots = crate::extensions::skills::Roots::build(
        None,
        None,
        plugin
            .skill_folders()
            .into_iter()
            .map(|folder| (manifest.name.clone(), folder))
            .collect(),
        None,
    );
    let (skills, _) = crate::extensions::skills::discover(&roots);
    Ok(Staged {
        manifest,
        staging,
        target,
        source: match source {
            Source::Git(url) => url.clone(),
            Source::Local(path) => path.display().to_string(),
        },
        skills,
    })
}

fn fetch(source: &Source, into: &Path) -> Result<()> {
    match source {
        Source::Git(url) => git_clone(url, into),
        Source::Local(folder) => {
            let mut budget = (0usize, 0u64);
            copy_tree(folder, into, &mut budget)
        }
    }
}

/// `git clone --depth 1`, without prompts, stopped after [`CLONE_TIMEOUT`].
fn git_clone(url: &str, into: &Path) -> Result<()> {
    use std::io::Read as _;
    let mut child = std::process::Command::new("git")
        .args([
            "-c",
            "protocol.ext.allow=never",
            "clone",
            "--depth",
            "1",
            "--quiet",
            "--no-tags",
            "--",
            url,
        ])
        .arg(into)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("running git (is Git installed?)")?;
    let mut stderr = child.stderr.take().context("reading git's output")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = (&mut stderr).take(8 * 1024).read_to_string(&mut text);
        text
    });
    let deadline = std::time::Instant::now() + CLONE_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for git")? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("git clone took too long and was stopped");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        let reason = output
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no details");
        bail!("git clone failed: {}", reason.trim());
    }
    Ok(())
}

/// Copies a folder, leaving out `.git` and links (so nothing outside it comes along).
fn copy_tree(from: &Path, to: &Path, budget: &mut (usize, u64)) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    let mut entries = std::fs::read_dir(from)
        .with_context(|| format!("reading {}", from.display()))?
        .flatten()
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let kind = entry.file_type().context("reading a file in the plugin")?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(&name), budget)?;
        } else if kind.is_file() {
            budget.0 += 1;
            budget.1 += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            if budget.0 > MAX_COPY_FILES || budget.1 > MAX_COPY_BYTES {
                bail!("the folder is too large for a plugin (over 2000 files or 50 MiB)");
            }
            std::fs::copy(entry.path(), to.join(&name))
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}

impl Staged {
    /// Everything the plugin adds, as lines for the confirmation. `built_in` says which command
    /// names are taken.
    pub(crate) fn review(&self, built_in: &dyn Fn(&str) -> bool) -> Vec<String> {
        let manifest = &self.manifest;
        let mut lines = vec![
            format!("{} {}", manifest.name, manifest.version),
            manifest.description.clone(),
            format!("From: {}", self.source),
            format!("Installs to: {}", self.target.display()),
            String::new(),
        ];
        lines.push(format!("Skills ({}):", self.skills.len()));
        for skill in &self.skills {
            lines.push(format!("  {}: {}", skill.name, skill.description));
        }
        lines.push(format!("Commands ({}):", manifest.commands.len()));
        for command in &manifest.commands {
            let taken = if built_in(&command.name) {
                " (a built-in command has this name, so it will not be available)"
            } else {
                ""
            };
            lines.push(format!(
                "  /{}: {}{taken}",
                command.name, command.description
            ));
            let first = command.prompt.lines().next().unwrap_or_default();
            lines.push(format!("    sends: {first}"));
        }
        lines.push(format!(
            "Mods ({}), programs that start with Cool Code and receive events:",
            manifest.mods.len()
        ));
        for entry in &manifest.mods {
            lines.push(format!("  {}: runs {}", entry.name, entry.command_line()));
            lines.push(format!("    in {}", self.target.display()));
            lines.push(format!(
                "    events: {}",
                if entry.events.is_empty() {
                    "none".to_owned()
                } else {
                    entry.events.join(", ")
                }
            ));
        }
        lines
    }

    /// Its mods as they will be once installed.
    pub(crate) fn mods(&self) -> Vec<ModInfo> {
        mods_in(&self.manifest, &self.target)
    }

    /// Moves the plugin into place. Refuses to replace anything.
    pub(crate) fn install(self) -> Result<PathBuf> {
        if std::fs::symlink_metadata(&self.target).is_ok() {
            bail!("a plugin named {} is already installed", self.manifest.name);
        }
        std::fs::rename(&self.staging, &self.target)
            .with_context(|| format!("moving the plugin to {}", self.target.display()))?;
        Ok(self.target.clone())
    }

    /// Deletes the staging folder.
    pub(crate) fn discard(self) {
        drop(self);
    }
}

/// A plugin that was not installed (the user said no, or never answered) leaves nothing
/// behind. After an install the staging folder no longer exists, so this does nothing.
impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

/// Deletes an installed plugin's folder.
pub(crate) fn remove(dirs: &Dirs, name: &str) -> Result<()> {
    if !crate::extensions::valid_name(name) {
        bail!("{name:?} is not a plugin name");
    }
    let plugins = plugins_dir(dirs)
        .and_then(|dir| dir.canonicalize().ok())
        .context("no plugins are installed")?;
    let target = plugins.join(name);
    let metadata = std::fs::symlink_metadata(&target)
        .with_context(|| format!("no plugin named {name} is installed"))?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(&target)
    } else {
        // A link is removed, never what it points to.
        std::fs::remove_file(&target)
    }
    .with_context(|| format!("removing {}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("coolcode-plugin-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("folder");
        path.canonicalize().expect("real path")
    }

    const MANIFEST: &str = r#"
name = "reviewer"
version = "1.2.0"
description = "Code review helpers"
skills = ["skills"]

[[commands]]
name = "review"
description = "Review the staged changes"
prompt = "Review the staged changes. Focus on $ARGUMENTS."

[[mods]]
name = "status"
command = "python3"
args = ["mods/status.py", "--every", "5 s"]
events = ["turn_finished", "tool_started"]
"#;

    /// A plugin folder ready to install.
    fn source_folder() -> PathBuf {
        let folder = temp("source");
        std::fs::write(folder.join("plugin.toml"), MANIFEST).unwrap();
        let skill = folder.join("skills/checklist");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: checklist\ndescription: A review checklist\n---\nCheck.\n",
        )
        .unwrap();
        std::fs::create_dir_all(folder.join("mods")).unwrap();
        std::fs::write(folder.join("mods/status.py"), "print('{}')\n").unwrap();
        folder
    }

    #[cfg(unix)]
    #[test]
    fn a_skill_folder_that_links_outside_the_plugin_is_refused_and_never_loaded() {
        let folder = source_folder();
        let outside = temp("outside");
        std::os::unix::fs::symlink(&outside, folder.join("linked")).unwrap();
        let manifest = manifest_with(|m| m.skills = vec!["linked".to_owned()]);
        let error = manifest
            .validate(&folder)
            .expect_err("a link out of the plugin");
        assert!(format!("{error:#}").contains("inside"), "{error:#}");
        let plugin = Plugin {
            manifest,
            dir: folder,
            enabled: true,
        };
        assert!(plugin.skill_folders().is_empty());
    }

    fn dirs() -> Dirs {
        Dirs::under(&temp("home"))
    }

    fn manifest_with(change: impl FnOnce(&mut Manifest)) -> Manifest {
        let mut manifest: Manifest = toml::from_str(MANIFEST).expect("parse");
        change(&mut manifest);
        manifest
    }

    #[test]
    fn a_manifest_names_the_plugin_and_everything_it_adds() {
        let folder = source_folder();
        let manifest = read_manifest(&folder).expect("valid");
        assert_eq!(manifest.name, "reviewer");
        assert_eq!(manifest.commands[0].name, "review");
        assert_eq!(manifest.mods[0].args[2], "5 s");
        for (bad, reason) in [
            (manifest_with(|m| m.name = "a/b".to_owned()), "name"),
            (manifest_with(|m| m.name = "..".to_owned()), "name"),
            (manifest_with(|m| m.version = String::new()), "version"),
            (
                manifest_with(|m| m.skills = vec!["../elsewhere".to_owned()]),
                "inside",
            ),
            (
                manifest_with(|m| m.skills = vec!["/etc".to_owned()]),
                "inside",
            ),
            (
                manifest_with(|m| m.skills = vec!["missing".to_owned()]),
                "missing",
            ),
            (
                manifest_with(|m| m.commands.push(m.commands[0].clone())),
                "twice",
            ),
            (
                manifest_with(|m| m.commands[0].prompt = String::new()),
                "prompt",
            ),
            (
                manifest_with(|m| m.mods[0].events = vec!["all".to_owned()]),
                "all",
            ),
            (manifest_with(|m| m.mods.push(m.mods[0].clone())), "twice"),
        ] {
            let error = bad.validate(&folder).expect_err(reason);
            assert!(format!("{error:#}").contains(reason), "{reason}: {error:#}");
        }
        std::fs::write(
            folder.join("plugin.toml"),
            format!("{MANIFEST}\nauto_approve = true\n"),
        )
        .unwrap();
        assert!(read_manifest(&folder).is_err(), "unknown keys are refused");
    }

    #[test]
    fn a_command_puts_the_users_words_where_the_prompt_asks_or_at_the_end() {
        let command = PluginCommand {
            name: "review".to_owned(),
            description: "d".to_owned(),
            prompt: "Review it. Focus on $ARGUMENTS.".to_owned(),
        };
        assert_eq!(command.expand("tests"), "Review it. Focus on tests.");
        let plain = PluginCommand {
            prompt: "Summarize the diff.".to_owned(),
            ..command
        };
        assert_eq!(plain.expand(""), "Summarize the diff.");
        assert_eq!(plain.expand("briefly"), "Summarize the diff.\n\nbriefly");
    }

    #[test]
    fn sources_are_git_addresses_or_local_folders() {
        let folder = source_folder();
        for git in [
            "https://github.com/someone/plugin",
            "git@github.com:someone/plugin.git",
            "ssh://git@host/plugin",
            "file:///tmp/plugin",
            "git://host/plugin",
        ] {
            assert_eq!(parse_source(git).expect(git), Source::Git(git.to_owned()));
        }
        assert_eq!(
            parse_source(&folder.display().to_string()).expect("local"),
            Source::Local(folder.clone())
        );
        assert!(parse_source("").is_err());
        assert!(
            parse_source("--upload-pack=evil").is_err(),
            "never an option"
        );
        assert!(parse_source("/no/such/folder").is_err());
    }

    #[test]
    fn staging_shows_everything_and_installs_nothing_until_confirmed() {
        let dirs = dirs();
        let staged = stage(&dirs, &Source::Local(source_folder())).expect("staged");
        let plugins = plugins_dir(&dirs).unwrap();
        assert!(staged.staging.starts_with(&plugins), "{:?}", staged.staging);
        assert_eq!(staged.target, plugins.join("reviewer"));
        assert!(!staged.target.exists(), "not installed yet");
        let review = staged.review(&|name| name == "help").join("\n");
        for expected in [
            "reviewer 1.2.0",
            "Code review helpers",
            "checklist: A review checklist",
            "/review: Review the staged changes",
            "python3 mods/status.py --every \"5 s\"",
            "turn_finished, tool_started",
        ] {
            assert!(review.contains(expected), "{expected} missing:\n{review}");
        }
        let (listed, _) = installed(&dirs, &crate::Settings::default());
        assert!(
            listed.is_empty(),
            "a staged plugin is not installed: {listed:?}"
        );
        let staging = staged.staging.clone();
        staged.discard();
        assert!(!staging.exists(), "saying no leaves nothing behind");
        assert!(!plugins.join("reviewer").exists());
    }

    #[test]
    fn a_staged_plugin_that_is_never_answered_leaves_nothing_behind() {
        let dirs = dirs();
        let staged = stage(&dirs, &Source::Local(source_folder())).expect("staged");
        let staging = staged.staging.clone();
        drop(staged);
        assert!(!staging.exists());
    }

    #[test]
    fn a_confirmed_plugin_is_installed_and_never_overwritten() {
        let dirs = dirs();
        let source = source_folder();
        let staged = stage(&dirs, &Source::Local(source.clone())).expect("staged");
        let mods = staged.mods();
        let target = staged.install().expect("installed");
        assert!(target.join("plugin.toml").is_file());
        assert_eq!(mods[0].id, "reviewer/status");
        assert_eq!(mods[0].dir, target, "approved for where it will run");
        let (listed, warnings) = installed(&dirs, &crate::Settings::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].enabled);
        assert_eq!(listed[0].mods(), mods);
        assert_eq!(listed[0].skill_folders(), [target.join("skills")]);
        let again = stage(&dirs, &Source::Local(source)).expect_err("already there");
        assert!(
            format!("{again:#}").contains("already installed"),
            "{again:#}"
        );
        let leftovers = std::fs::read_dir(plugins_dir(&dirs).unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(leftovers, 0, "the refused staging folder was removed");
        let mut settings = crate::Settings::default();
        settings.disabled_plugins.push("reviewer".to_owned());
        assert!(!installed(&dirs, &settings).0[0].enabled);
    }

    #[test]
    fn removing_checks_the_name_and_deletes_only_that_plugin() {
        let dirs = dirs();
        let target = stage(&dirs, &Source::Local(source_folder()))
            .expect("staged")
            .install()
            .expect("installed");
        let neighbour = dirs.coolcode.as_ref().unwrap().join("keep.txt");
        std::fs::write(&neighbour, "keep").unwrap();
        for bad in ["../keep.txt", "a/b", "..", "", "missing"] {
            assert!(remove(&dirs, bad).is_err(), "{bad}");
        }
        assert!(neighbour.exists());
        remove(&dirs, "reviewer").expect("removed");
        assert!(!target.exists());
        assert!(neighbour.exists());
    }

    #[cfg(unix)]
    #[test]
    fn links_in_a_local_folder_are_not_copied() {
        let dirs = dirs();
        let source = source_folder();
        let secret = temp("secret").join("id_file");
        std::fs::write(&secret, "private").unwrap();
        std::os::unix::fs::symlink(&secret, source.join("link")).unwrap();
        let staged = stage(&dirs, &Source::Local(source)).expect("staged");
        assert!(!staged.staging.join("link").exists());
        staged.discard();
    }

    #[test]
    fn installed_lists_valid_plugins_and_reports_broken_ones() {
        let dirs = dirs();
        let plugins = plugins_dir(&dirs).unwrap();
        std::fs::create_dir_all(plugins.join("broken")).unwrap();
        std::fs::write(plugins.join("broken/plugin.toml"), "name = 5").unwrap();
        std::fs::create_dir_all(plugins.join(".staging-x")).unwrap();
        stage(&dirs, &Source::Local(source_folder()))
            .unwrap()
            .install()
            .unwrap();
        let (listed, warnings) = installed(&dirs, &crate::Settings::default());
        assert_eq!(listed.len(), 1);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("broken"), "{warnings:?}");
    }

    #[test]
    fn a_plugin_can_be_cloned_from_a_git_repository() {
        let repository = source_folder();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
                .args(args)
                .current_dir(&repository)
                .output()
                .expect("git runs")
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["add", "."]).status.success());
        assert!(git(&["commit", "-q", "-m", "plugin"]).status.success());
        // file:///C:/path on Windows (without the \\?\ prefix), file:///path elsewhere.
        let path = repository.display().to_string().replace('\\', "/");
        let path = path.trim_start_matches("//?/").trim_start_matches('/');
        let url = format!("file:///{path}");
        let dirs = dirs();
        let staged = stage(&dirs, &Source::Git(url)).expect("cloned");
        assert_eq!(staged.manifest.name, "reviewer");
        assert!(staged.staging.join("skills/checklist/SKILL.md").is_file());
        staged.discard();
        assert!(stage(&dirs, &Source::Git("file:///no/such/repository".to_owned())).is_err());
    }
}
