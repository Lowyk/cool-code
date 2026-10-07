//! What the harness remembers about each project folder: whether it is trusted, and whether
//! `CLAUDE.md` / `AGENTS.md` should be loaded there.
//!
//! Everything lives in one file under `~/.coolcode/`, keyed by the folder's real path. Nothing is
//! stored inside a project, so a repository cannot ship its own trust (a cloned repo can contain
//! any file at all, including a fake "trusted" marker), and every folder can be listed and reset.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectEntry {
    /// The folder's normalized path (see [`key_for`]).
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) trusted: bool,
    /// `None` follows the global default; `Some` is this project's own choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) load_claude_md: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) load_agents_md: Option<bool>,
}

impl ProjectEntry {
    /// True when the entry carries no information and can be dropped from the file.
    fn is_blank(&self) -> bool {
        !self.trusted && self.load_claude_md.is_none() && self.load_agents_md.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Registry {
    #[serde(default, rename = "project")]
    pub(crate) projects: Vec<ProjectEntry>,
}

#[cfg(not(test))]
pub(crate) fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".coolcode")
        .join("projects.toml")
}

// Every call gets its own file so tests never touch the real registry or each other.
#[cfg(test)]
pub(crate) fn default_path() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    std::env::temp_dir()
        .join(format!(
            "harness-test-{}-{started}-projects-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
        .join("projects.toml")
}

/// The registry key for a folder: its real path, without the Windows `\\?\` prefix, and
/// case-folded on Windows where paths are case-insensitive.
pub(crate) fn key_for(root: &Path) -> String {
    let real = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let text = real.to_string_lossy().into_owned();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

impl Registry {
    /// Reads the registry. A missing file is an empty registry; an unreadable one is moved aside
    /// (so it is never silently overwritten) and treated as empty.
    pub(crate) fn load_from(path: &Path) -> Registry {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Registry::default();
        };
        match toml::from_str(&text) {
            Ok(registry) => registry,
            Err(_) => {
                let mut aside = path.as_os_str().to_owned();
                aside.push(".corrupt");
                let _ = std::fs::rename(path, PathBuf::from(aside));
                Registry::default()
            }
        }
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut kept = self.clone();
        kept.projects.retain(|entry| !entry.is_blank());
        kept.projects.sort_by(|a, b| a.path.cmp(&b.path));
        let text = toml::to_string_pretty(&kept).context("serializing the project registry")?;
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text)
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, path).with_context(|| format!("saving {}", path.display()))
    }

    pub(crate) fn entry(&self, root: &Path) -> Option<&ProjectEntry> {
        let key = key_for(root);
        self.projects.iter().find(|entry| entry.path == key)
    }

    fn entry_mut(&mut self, root: &Path) -> &mut ProjectEntry {
        let key = key_for(root);
        match self.projects.iter().position(|entry| entry.path == key) {
            Some(index) => &mut self.projects[index],
            None => {
                self.projects.push(ProjectEntry {
                    path: key,
                    ..ProjectEntry::default()
                });
                self.projects.last_mut().expect("just pushed")
            }
        }
    }

    pub(crate) fn is_trusted(&self, root: &Path) -> bool {
        self.entry(root).is_some_and(|entry| entry.trusted)
    }

    pub(crate) fn set_trusted(&mut self, root: &Path, trusted: bool) {
        self.entry_mut(root).trusted = trusted;
    }

    /// Whether `CLAUDE.md` is loaded for this project, given the global default.
    pub(crate) fn loads_claude_md(&self, root: &Path, default: bool) -> bool {
        self.entry(root)
            .and_then(|entry| entry.load_claude_md)
            .unwrap_or(default)
    }

    pub(crate) fn loads_agents_md(&self, root: &Path, default: bool) -> bool {
        self.entry(root)
            .and_then(|entry| entry.load_agents_md)
            .unwrap_or(default)
    }

    pub(crate) fn set_claude_md(&mut self, root: &Path, load: bool) {
        self.entry_mut(root).load_claude_md = Some(load);
    }

    pub(crate) fn set_agents_md(&mut self, root: &Path, load: bool) {
        self.entry_mut(root).load_agents_md = Some(load);
    }

    /// Forgets every trusted folder (other per-project choices stay).
    pub(crate) fn clear_trusted(&mut self) {
        for entry in &mut self.projects {
            entry.trusted = false;
        }
    }

    pub(crate) fn trusted_count(&self) -> usize {
        self.projects.iter().filter(|entry| entry.trusted).count()
    }
}

// Convenience wrappers: each reads the file fresh so two running windows never overwrite each
// other with stale data.

pub(crate) fn is_trusted_at(registry: &Path, root: &Path) -> bool {
    Registry::load_from(registry).is_trusted(root)
}

pub(crate) fn update(registry: &Path, change: impl FnOnce(&mut Registry)) -> Result<()> {
    let mut loaded = Registry::load_from(registry);
    change(&mut loaded);
    loaded.save_to(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "harness-projects-test-{}-{name}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("folder");
        path
    }

    fn file() -> PathBuf {
        default_path()
    }

    #[test]
    fn an_unknown_folder_is_untrusted_and_follows_the_defaults() {
        let registry = Registry::default();
        let root = folder("unknown");
        assert!(!registry.is_trusted(&root));
        assert!(registry.loads_claude_md(&root, true));
        assert!(!registry.loads_claude_md(&root, false));
        assert!(!registry.loads_agents_md(&root, false));
    }

    #[test]
    fn trust_is_saved_per_folder_and_survives_a_reload() {
        let (path, a, b) = (file(), folder("a"), folder("b"));
        update(&path, |registry| registry.set_trusted(&a, true)).expect("save");
        assert!(is_trusted_at(&path, &a));
        assert!(!is_trusted_at(&path, &b), "other folders are unaffected");
        update(&path, |registry| registry.set_trusted(&a, false)).expect("revoke");
        assert!(!is_trusted_at(&path, &a));
    }

    #[test]
    fn different_spellings_of_one_folder_are_the_same_project() {
        let (path, root) = (file(), folder("spelling"));
        update(&path, |registry| registry.set_trusted(&root, true)).expect("save");
        assert!(is_trusted_at(&path, &root.join(".")));
        assert!(is_trusted_at(&path, &root.join("sub").join("..")) || !root.join("sub").exists());
        assert_eq!(key_for(&root), key_for(&root.join(".")));
    }

    #[test]
    fn a_cloned_repo_cannot_trust_itself() {
        // Old versions kept trust in <project>/.coolcode/trusted. A repository can ship that
        // file, so it must carry no weight at all.
        let (path, root) = (file(), folder("malicious"));
        std::fs::create_dir_all(root.join(".coolcode")).expect("dir");
        std::fs::write(root.join(".coolcode").join("trusted"), "trusted\n").expect("marker");
        std::fs::write(root.join("projects.toml"), "[[project]]\ntrusted = true\n").expect("fake");
        assert!(!is_trusted_at(&path, &root));
    }

    #[test]
    fn the_per_project_switches_override_the_default_either_way() {
        let (path, root) = (file(), folder("switches"));
        update(&path, |registry| {
            registry.set_claude_md(&root, true);
            registry.set_agents_md(&root, false);
        })
        .expect("save");
        let registry = Registry::load_from(&path);
        assert!(registry.loads_claude_md(&root, false), "forced on");
        assert!(!registry.loads_agents_md(&root, true), "forced off");
    }

    #[test]
    fn clearing_trust_keeps_the_other_choices() {
        let (path, root) = (file(), folder("clear"));
        update(&path, |registry| {
            registry.set_trusted(&root, true);
            registry.set_claude_md(&root, true);
        })
        .expect("save");
        update(&path, Registry::clear_trusted).expect("clear");
        let registry = Registry::load_from(&path);
        assert!(!registry.is_trusted(&root));
        assert!(registry.loads_claude_md(&root, false));
        assert_eq!(registry.trusted_count(), 0);
    }

    #[test]
    fn empty_entries_are_not_written() {
        let (path, root) = (file(), folder("blank"));
        update(&path, |registry| registry.set_trusted(&root, true)).expect("save");
        update(&path, |registry| registry.set_trusted(&root, false)).expect("revoke");
        let text = std::fs::read_to_string(&path).expect("file");
        assert!(!text.contains("[[project]]"), "{text}");
    }

    #[test]
    fn a_corrupt_registry_is_set_aside_not_overwritten() {
        let path = file();
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        std::fs::write(&path, "this is [not toml").expect("corrupt");
        let root = folder("recover");
        assert!(!is_trusted_at(&path, &root), "unreadable means untrusted");
        assert!(
            path.with_extension("toml.corrupt").exists(),
            "the bad file is kept for inspection"
        );
        update(&path, |registry| registry.set_trusted(&root, true)).expect("fresh start");
        assert!(is_trusted_at(&path, &root));
    }

    #[test]
    fn two_windows_do_not_overwrite_each_other() {
        let (path, a, b) = (file(), folder("w-a"), folder("w-b"));
        update(&path, |registry| registry.set_trusted(&a, true)).expect("window one");
        update(&path, |registry| registry.set_trusted(&b, true)).expect("window two");
        let registry = Registry::load_from(&path);
        assert!(registry.is_trusted(&a) && registry.is_trusted(&b));
        assert_eq!(registry.trusted_count(), 2);
    }
}
