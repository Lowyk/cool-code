//! Ways to extend Cool Code without changing it: skills (instructions the model can load),
//! plugins (folders that bundle skills, commands and mods) and mods (programs that watch what
//! happens and show a status or a notice).
//!
//! None of them can change the permission mode, approve an action or bypass a check: skills and
//! plugin commands are text for the model, and mods only receive events and send back text.

use std::path::PathBuf;

pub(crate) mod skills;

/// `~/.coolcode`, where the user's own skills, plugins and mods live.
#[cfg(not(test))]
pub(crate) fn coolcode_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".coolcode"))
}

/// Tests never look at the real home folder; this one stays empty.
#[cfg(test)]
pub(crate) fn coolcode_dir() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("harness-test-{}-coolcode-home", std::process::id())))
}

/// `~/.claude`, for the opt-in Claude skills.
#[cfg(not(test))]
pub(crate) fn claude_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude"))
}

#[cfg(test)]
pub(crate) fn claude_dir() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("harness-test-{}-claude-home", std::process::id())))
}

/// The two home folders extensions are read from. Tests point them at temporary folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dirs {
    /// `~/.coolcode`
    pub(crate) coolcode: Option<PathBuf>,
    /// `~/.claude`
    pub(crate) claude: Option<PathBuf>,
}

impl Dirs {
    pub(crate) fn current() -> Dirs {
        Dirs {
            coolcode: coolcode_dir(),
            claude: claude_dir(),
        }
    }

    /// Everything under one folder, for tests.
    #[cfg(test)]
    pub(crate) fn under(root: &std::path::Path) -> Dirs {
        Dirs {
            coolcode: Some(root.join(".coolcode")),
            claude: Some(root.join(".claude")),
        }
    }

    /// `~/.coolcode/<name>`
    pub(crate) fn coolcode_join(&self, name: &str) -> Option<PathBuf> {
        self.coolcode.as_ref().map(|dir| dir.join(name))
    }
}

/// Longest name of a skill, plugin, mod or command.
pub(crate) const MAX_NAME: usize = 64;

/// Whether `name` can name a skill, plugin, mod or command: lowercase letters, digits, `-` and
/// `_`, starting with a letter or digit. This keeps names usable as `/name` and as one folder
/// name, with no path separators, dots or spaces.
pub(crate) fn valid_name(name: &str) -> bool {
    let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    name.len() <= MAX_NAME
        && name.chars().next().is_some_and(plain)
        && name.chars().all(|c| plain(c) || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_lowercase_words_without_separators() {
        for good in ["review", "pdf-tools", "a", "x_1", "9lives"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in [
            "",
            "Review",
            "a b",
            "../x",
            "a/b",
            "a\\b",
            ".hidden",
            "-dash",
            "_under",
            "dot.name",
            "ünï",
            &"x".repeat(MAX_NAME + 1),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
