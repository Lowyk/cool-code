//! `/copy`: putting text on the clipboard. The system's own tool is tried first (`pbcopy`,
//! `wl-copy`, `xclip`, `xsel`); without one the terminal is asked to do it with an OSC 52
//! escape sequence, which most modern terminals (Windows Terminal, iTerm2, kitty, WezTerm,
//! Alacritty, tmux with its option on) obey and some (Terminal.app) ignore.

use base64::Engine as _;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The most text sent in an OSC 52 sequence; terminals refuse longer ones.
const MAX_OSC_BYTES: usize = 100_000;

/// How the text got to the clipboard.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Copied {
    /// A program did it.
    Tool(String),
    /// The terminal is asked to: write this to it.
    Terminal(String),
}

/// Runs a program with `text` on its standard input and says whether it succeeded.
pub(super) trait Runner {
    fn run(&self, program: &str, args: &[&str], text: &str) -> bool;
}

/// The real thing: starts the program, feeds it the text and waits at most two seconds.
pub(super) struct System;

impl Runner for System {
    fn run(&self, program: &str, args: &[&str], text: &str) -> bool {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        // Dropping the pipe at the end of this block tells the program the text is complete.
        if let Some(mut stdin) = child.stdin.take()
            && stdin.write_all(text.as_bytes()).is_err()
        {
            let _ = child.kill();
            return false;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    }
}

/// The programs to try on `os`, best first. Windows has none: its `clip` mangles non-ASCII
/// text, and Windows Terminal obeys OSC 52.
fn tools(os: &str, wayland: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    match os {
        "macos" => vec![("pbcopy", vec![])],
        "windows" => Vec::new(),
        _ => {
            let mut found = Vec::new();
            if wayland {
                found.push(("wl-copy", vec![]));
            }
            found.push(("xclip", vec!["-selection", "clipboard"]));
            found.push(("xsel", vec!["--clipboard", "--input"]));
            found
        }
    }
}

/// Copies `text`: with the first tool that works, else by an escape sequence for the terminal.
pub(super) fn copy(
    text: &str,
    runner: &dyn Runner,
    os: &str,
    wayland: bool,
) -> Result<Copied, String> {
    for (program, args) in tools(os, wayland) {
        if runner.run(program, &args, text) {
            return Ok(Copied::Tool(program.to_owned()));
        }
    }
    if text.len() > MAX_OSC_BYTES {
        return Err(
            "That is too long to hand to the terminal and no clipboard program was found. Use /export instead."
                .to_owned(),
        );
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    Ok(Copied::Terminal(format!("\u{1b}]52;c;{encoded}\u{7}")))
}

/// The code blocks of a Markdown answer, without their fences.
pub(super) fn code_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            match current.take() {
                Some(lines) => blocks.push(lines.join("\n")),
                None => current = Some(Vec::new()),
            }
        } else if let Some(lines) = current.as_mut() {
            lines.push(line);
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Succeeds only for the programs it is told about and records every try.
    struct Fake {
        works: Vec<&'static str>,
        tried: RefCell<Vec<(String, String)>>,
    }

    impl Fake {
        fn new(works: &[&'static str]) -> Fake {
            Fake {
                works: works.to_vec(),
                tried: RefCell::new(Vec::new()),
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, program: &str, args: &[&str], text: &str) -> bool {
            self.tried.borrow_mut().push((
                format!("{program} {}", args.join(" ")).trim().to_owned(),
                text.to_owned(),
            ));
            self.works.contains(&program)
        }
    }

    #[test]
    fn the_first_tool_that_works_is_used_and_the_rest_are_not_tried() {
        let fake = Fake::new(&["xclip", "xsel"]);
        let copied = copy("hello", &fake, "linux", true).unwrap();
        assert_eq!(copied, Copied::Tool("xclip".to_owned()));
        let tried = fake.tried.borrow();
        let programs = tried
            .iter()
            .map(|(call, _)| call.as_str())
            .collect::<Vec<_>>();
        assert_eq!(programs, ["wl-copy", "xclip -selection clipboard"]);
        assert!(tried.iter().all(|(_, text)| text == "hello"));
        let mac = Fake::new(&["pbcopy"]);
        assert_eq!(
            copy("x", &mac, "macos", false).unwrap(),
            Copied::Tool("pbcopy".to_owned())
        );
        let x11 = Fake::new(&["xsel"]);
        copy("x", &x11, "linux", false).unwrap();
        assert!(
            !x11.tried
                .borrow()
                .iter()
                .any(|(call, _)| call.starts_with("wl-copy"))
        );
    }

    #[test]
    fn without_a_working_tool_the_terminal_is_asked_with_an_osc_52_sequence() {
        let fake = Fake::new(&[]);
        let Copied::Terminal(escape) = copy("héllo", &fake, "linux", false).unwrap() else {
            panic!("expected the terminal");
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode("héllo");
        assert_eq!(escape, format!("\u{1b}]52;c;{encoded}\u{7}"));
        let windows = Fake::new(&["clip"]);
        assert!(matches!(
            copy("x", &windows, "windows", false).unwrap(),
            Copied::Terminal(_)
        ));
        assert!(
            windows.tried.borrow().is_empty(),
            "Windows tries no program"
        );
    }

    #[test]
    fn text_too_long_for_the_terminal_is_refused_with_a_way_out() {
        let fake = Fake::new(&[]);
        let error = copy(&"x".repeat(MAX_OSC_BYTES + 1), &fake, "linux", false).unwrap_err();
        assert!(error.contains("/export"), "{error}");
        assert!(copy(&"x".repeat(MAX_OSC_BYTES), &fake, "linux", false).is_ok());
        // A tool has no such limit.
        let tool = Fake::new(&["xclip"]);
        assert!(copy(&"x".repeat(MAX_OSC_BYTES * 3), &tool, "linux", false).is_ok());
    }

    #[test]
    fn code_blocks_are_found_without_their_fences() {
        let answer =
            "Intro\n```rust\nfn main() {}\n```\ntext\n  ```\nsecond\nblock\n```\n```\nunclosed";
        assert_eq!(code_blocks(answer), ["fn main() {}", "second\nblock"]);
        assert!(code_blocks("no code here").is_empty());
    }
}
