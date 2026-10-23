//! `/copy`, `/export`, `/retry` and `/rename`: small things to do with the conversation.

use crate::tui::clipboard::{self, Copied, Runner};
use crate::tui::state::{App, TranscriptKind};
use anyhow::{Context, Result, bail};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Longest session title.
const MAX_TITLE_CHARS: usize = 80;

impl App {
    /// `/copy [n]`: the last answer, or the nth code block of it, on the clipboard.
    pub(in crate::tui) fn copy_command(&mut self, argument: &str) -> String {
        self.copy_with(
            argument,
            &clipboard::System,
            std::env::consts::OS,
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
        )
    }

    fn copy_with(
        &mut self,
        argument: &str,
        runner: &dyn Runner,
        os: &str,
        wayland: bool,
    ) -> String {
        let Some(answer) = self
            .transcript
            .iter()
            .rev()
            .find(|entry| entry.kind == TranscriptKind::Assistant && !entry.text.trim().is_empty())
            .map(|entry| entry.text.clone())
        else {
            return "There is no answer to copy yet.".to_owned();
        };
        let (text, what) = if argument.is_empty() {
            (answer, "the answer".to_owned())
        } else {
            let Ok(number) = argument.parse::<usize>() else {
                return "Usage: /copy [n] where n is the number of a code block in the answer."
                    .to_owned();
            };
            let blocks = clipboard::code_blocks(&answer);
            match number.checked_sub(1).and_then(|index| blocks.get(index)) {
                Some(block) => (block.clone(), format!("code block {number}")),
                None => {
                    return format!(
                        "The answer has {} code block(s); there is no number {number}.",
                        blocks.len()
                    );
                }
            }
        };
        match clipboard::copy(&text, runner, os, wayland) {
            Ok(Copied::Tool(program)) => format!("Copied {what} with {program}."),
            Ok(Copied::Terminal(escape)) => {
                self.clipboard_escape = Some(escape);
                format!(
                    "Asked the terminal to copy {what}. If nothing arrives, your terminal does not support that: use /export instead."
                )
            }
            Err(error) => error,
        }
    }

    /// The conversation as Markdown.
    pub(in crate::tui) fn markdown_export(&self, exported: &str) -> String {
        let mut text = format!(
            "# {}\n\n*Exported from Cool Code {} on {exported}.*\n",
            self.session_title(),
            env!("CARGO_PKG_VERSION")
        );
        for entry in &self.transcript {
            match entry.kind {
                TranscriptKind::User => {
                    text.push_str(&format!("\n## You\n\n{}\n", entry.text.trim_end()));
                }
                TranscriptKind::Assistant => {
                    text.push_str(&format!("\n## Assistant\n\n{}\n", entry.text.trim_end()));
                    if let Some(footer) = &entry.footer {
                        text.push_str(&format!(
                            "\n*{} · {}*\n",
                            crate::policy::mode_label(&footer.mode),
                            footer.details
                        ));
                    }
                }
                TranscriptKind::CommandOutput => {
                    text.push('\n');
                    for line in entry.text.lines() {
                        text.push_str(&format!("> {line}\n"));
                    }
                }
                TranscriptKind::Error => {
                    text.push('\n');
                    for (index, line) in entry.text.lines().enumerate() {
                        let label = if index == 0 { "**Error:** " } else { "" };
                        text.push_str(&format!("> {label}{line}\n"));
                    }
                }
            }
        }
        text
    }

    /// `/export [file]`: writes the conversation to a file that does not exist yet.
    pub(in crate::tui) fn export_command(&mut self, argument: &str) -> Result<String> {
        if self.transcript.is_empty() {
            return Ok("There is no conversation to export yet.".to_owned());
        }
        let now = chrono::Local::now();
        let target = match export_target(
            self.extensions.dirs.coolcode_join("exports"),
            &self.session_title(),
            argument,
            &now.format("%Y%m%d-%H%M%S").to_string(),
        ) {
            Ok(target) => target,
            Err(error) => return Ok(format!("Could not export: {error:#}")),
        };
        let text = self.markdown_export(&now.format("%Y-%m-%d").to_string());
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        // `create_new` refuses a path that exists, including a link, so nothing is overwritten.
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        Ok(match written {
            Ok(()) => format!("Exported the conversation to {}.", target.display()),
            Err(error) => format!("Could not export to {}: {error}.", target.display()),
        })
    }

    /// `/retry`'s bookkeeping: forgets the last request and everything after it and returns the
    /// request's text, ready to be sent again.
    pub(in crate::tui) fn retry_prepare(&mut self) -> Option<String> {
        let at = self.transcript.iter().rposition(|entry| {
            entry.kind == TranscriptKind::User && !entry.text.starts_with('/')
        })?;
        let text = self.transcript[at].text.clone();
        self.transcript.truncate(at);
        if let Some(message) = self
            .messages
            .iter()
            .rposition(|message| message.role == "user")
        {
            self.messages.truncate(message);
        }
        self.history_scroll = 0;
        Some(text)
    }

    /// `/retry`: sends the last request again.
    pub(in crate::tui) fn retry_command(&mut self) -> Result<String> {
        if self.pending.is_some() {
            return Ok("Wait for the current answer to finish first.".to_owned());
        }
        let Some(text) = self.retry_prepare() else {
            return Ok("There is no request to send again.".to_owned());
        };
        self.send_prompt(text, &std::collections::HashSet::new())?;
        Ok(
            "Sent the last request again. Changes it made to files stay; /undo takes them back."
                .to_owned(),
        )
    }

    /// `/rename <title>`: names the session.
    pub(in crate::tui) fn rename_command(&mut self, argument: &str) -> String {
        let title = argument.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            return "Usage: /rename <title>".to_owned();
        }
        let title = if title.chars().count() > MAX_TITLE_CHARS {
            format!(
                "{}…",
                title.chars().take(MAX_TITLE_CHARS - 1).collect::<String>()
            )
        } else {
            title
        };
        self.custom_title = Some(title.clone());
        self.save_session();
        format!("This session is now called {title}.")
    }
}

/// Where `/export` writes: a path the user gave (which must not exist), else a new file in the
/// exports folder named after the session.
fn export_target(
    folder: Option<PathBuf>,
    title: &str,
    given: &str,
    stamp: &str,
) -> Result<PathBuf> {
    let given = given.trim();
    if given.is_empty() {
        let folder = folder.context("there is no home folder to keep exports in")?;
        return Ok(folder.join(format!("{}-{stamp}.md", slug(title))));
    }
    let path = match given.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .context("could not find the home folder")?
            .join(rest),
        None => PathBuf::from(given),
    };
    if std::fs::symlink_metadata(&path).is_ok() {
        bail!("{} already exists; choose another name", path.display());
    }
    if path.file_name().is_none() || path.to_string_lossy().ends_with(['/', '\\']) {
        bail!("{given} is not a file name");
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent
        && !Path::new(parent).is_dir()
    {
        bail!("the folder {} does not exist", parent.display());
    }
    Ok(path)
}

/// A title as a file name: lowercase letters and digits joined by single dashes.
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let cut: String = slug.chars().take(40).collect();
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "conversation".to_owned()
    } else {
        cut.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::Dirs;
    use crate::tui::state::{AnswerFooter, TranscriptEntry};
    use std::cell::RefCell;

    fn entry(kind: TranscriptKind, text: &str) -> TranscriptEntry {
        TranscriptEntry {
            kind,
            text: text.to_owned(),
            footer: None,
        }
    }

    fn app() -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app
    }

    struct Fake {
        works: bool,
        tried: RefCell<Vec<String>>,
    }

    impl Runner for Fake {
        fn run(&self, program: &str, _: &[&str], text: &str) -> bool {
            self.tried.borrow_mut().push(format!("{program}:{text}"));
            self.works
        }
    }

    fn fake(works: bool) -> Fake {
        Fake {
            works,
            tried: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn copy_takes_the_last_answer_or_a_numbered_code_block() {
        let mut app = app();
        assert!(
            app.copy_with("", &fake(true), "linux", false)
                .contains("no answer")
        );
        app.transcript.push(entry(TranscriptKind::User, "q"));
        app.transcript.push(entry(TranscriptKind::Assistant, "old"));
        app.transcript.push(entry(
            TranscriptKind::Assistant,
            "Here:\n```\nfirst\n```\nand\n```\nsecond\n```\n",
        ));
        app.transcript
            .push(entry(TranscriptKind::CommandOutput, "/help output"));
        let runner = fake(true);
        let said = app.copy_with("", &runner, "macos", false);
        assert_eq!(said, "Copied the answer with pbcopy.");
        assert!(runner.tried.borrow()[0].contains("Here:"));
        let runner = fake(true);
        assert_eq!(
            app.copy_with("2", &runner, "macos", false),
            "Copied code block 2 with pbcopy."
        );
        assert_eq!(runner.tried.borrow()[0], "pbcopy:second");
        assert!(
            app.copy_with("3", &fake(true), "macos", false)
                .contains("2 code block(s)")
        );
        assert!(
            app.copy_with("0", &fake(true), "macos", false)
                .contains("no number 0")
        );
        assert!(
            app.copy_with("x", &fake(true), "macos", false)
                .contains("Usage: /copy")
        );
    }

    #[test]
    fn without_a_tool_the_escape_is_queued_for_the_terminal() {
        let mut app = app();
        app.transcript
            .push(entry(TranscriptKind::Assistant, "hello"));
        let said = app.copy_with("", &fake(false), "linux", false);
        assert!(said.contains("Asked the terminal"), "{said}");
        let escape = app.clipboard_escape.take().expect("queued");
        assert!(escape.starts_with("\u{1b}]52;c;") && escape.ends_with('\u{7}'));
    }

    #[test]
    fn the_export_is_markdown_with_each_part_in_its_place() {
        let mut app = app();
        app.transcript
            .push(entry(TranscriptKind::User, "fix the build"));
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Assistant,
            text: "Done.\n".to_owned(),
            footer: Some(AnswerFooter {
                mode: "manual".to_owned(),
                details: "Some Model · 2.0s".to_owned(),
            }),
        });
        app.transcript.push(entry(
            TranscriptKind::CommandOutput,
            "ran cargo test\n12 passed",
        ));
        app.transcript
            .push(entry(TranscriptKind::Error, "Request failed\nbad key"));
        let text = app.markdown_export("2026-10-23");
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            text,
            format!(
                "# fix the build\n\n*Exported from Cool Code {version} on 2026-10-23.*\n\n## You\n\nfix the build\n\n## Assistant\n\nDone.\n\n*Manual · Some Model · 2.0s*\n\n> ran cargo test\n> 12 passed\n\n> **Error:** Request failed\n> bad key\n"
            )
        );
    }

    #[test]
    fn exporting_writes_a_new_file_and_never_overwrites_one() {
        let home = std::env::temp_dir().join(format!("coolcode-export-{}", uuid::Uuid::new_v4()));
        let mut app = app();
        app.extensions.dirs = Dirs::under(&home);
        assert!(app.export_command("").unwrap().contains("no conversation"));
        app.transcript
            .push(entry(TranscriptKind::User, "Fix: the *build*!"));
        let said = app.export_command("").unwrap();
        assert!(said.starts_with("Exported the conversation to "), "{said}");
        let exported = std::fs::read_dir(home.join(".coolcode/exports"))
            .unwrap()
            .flatten()
            .next()
            .unwrap();
        let name = exported.file_name().to_string_lossy().into_owned();
        assert!(
            name.starts_with("fix-the-build-") && name.ends_with(".md"),
            "{name}"
        );
        let target = home.join("mine.md");
        let said = app.export_command(&target.display().to_string()).unwrap();
        assert!(said.contains("Exported"), "{said}");
        std::fs::write(&target, "precious").unwrap();
        let said = app.export_command(&target.display().to_string()).unwrap();
        assert!(said.contains("already exists"), "{said}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious");
        let said = app
            .export_command(&home.join("missing/dir/x.md").display().to_string())
            .unwrap();
        assert!(said.contains("does not exist"), "{said}");
        #[cfg(unix)]
        {
            let link = home.join("link.md");
            std::os::unix::fs::symlink(home.join("elsewhere.md"), &link).unwrap();
            let said = app.export_command(&link.display().to_string()).unwrap();
            assert!(
                said.contains("already exists"),
                "a link is not followed: {said}"
            );
            assert!(!home.join("elsewhere.md").exists());
        }
    }

    #[test]
    fn titles_become_plain_file_names() {
        assert_eq!(slug("Fix: the *build*!"), "fix-the-build");
        assert_eq!(slug("  ---  "), "conversation");
        assert_eq!(
            slug("ÜBER cool"),
            "ber-cool",
            "only ASCII letters and digits are kept"
        );
        assert!(slug(&"word ".repeat(30)).len() <= 40);
    }

    #[test]
    fn retry_drops_the_last_request_and_what_followed_and_returns_it() {
        let mut app = app();
        assert_eq!(app.retry_prepare(), None);
        for (kind, text) in [
            (TranscriptKind::User, "first"),
            (TranscriptKind::Assistant, "one"),
            (TranscriptKind::User, "/help"),
            (TranscriptKind::CommandOutput, "help text"),
            (TranscriptKind::User, "second"),
            (TranscriptKind::CommandOutput, "tool ran"),
            (TranscriptKind::Assistant, "two"),
            (TranscriptKind::User, "/usage"),
            (TranscriptKind::CommandOutput, "usage text"),
        ] {
            app.transcript.push(entry(kind, text));
        }
        for text in ["first", "second"] {
            app.messages
                .push(crate::provider::ChatMessage::user_with_images(
                    text.to_owned(),
                    text.to_owned(),
                    Vec::new(),
                ));
            app.messages
                .push(crate::provider::ChatMessage::assistant(format!(
                    "re: {text}"
                )));
        }
        assert_eq!(app.retry_prepare().as_deref(), Some("second"));
        let texts = app
            .transcript
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(texts, ["first", "one", "/help", "help text"]);
        assert_eq!(
            app.messages.len(),
            2,
            "the last request and its answer are gone"
        );
        assert_eq!(app.messages[0].display, "first");
    }

    #[test]
    fn retry_waits_for_a_running_answer_and_needs_a_request() {
        let mut app = app();
        assert!(app.retry_command().unwrap().contains("no request"));
        app.transcript.push(entry(TranscriptKind::User, "ask"));
        let (_sender, receiver) = std::sync::mpsc::channel();
        app.pending = Some(receiver);
        assert!(app.retry_command().unwrap().contains("Wait"));
        assert_eq!(app.transcript.len(), 1, "nothing was dropped");
    }

    #[test]
    fn rename_sets_a_title_that_wins_over_the_first_prompt_and_is_saved() {
        let mut app = app();
        app.transcript
            .push(entry(TranscriptKind::User, "first prompt"));
        assert_eq!(app.session_title(), "first prompt");
        assert!(app.rename_command("   ").contains("Usage"));
        let said = app.rename_command("  Release   prep ");
        assert_eq!(said, "This session is now called Release prep.");
        assert_eq!(app.session_title(), "Release prep");
        let long = app.rename_command(&"w".repeat(200));
        assert!(long.chars().count() < 120, "{long}");
        assert!(app.session_title().ends_with('…'));
    }
}
