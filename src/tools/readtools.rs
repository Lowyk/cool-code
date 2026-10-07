//! The read-only tools the model explores a repository with: listing, reading line ranges,
//! searching, and looking at Git history and changes.
//!
//! They are built to answer the questions a person would ask while getting oriented, in as little
//! output as possible: scope a listing to a folder or a glob, read just the lines that matter
//! (numbered, because edits address lines), and search with context.

use super::{
    MAX_FILES, MAX_OUTPUT_BYTES, MAX_READ_BYTES, canonical_root, collect_files,
    validate_relative_path,
};
use anyhow::{Context, Result, bail};
use regex::RegexBuilder;
use std::path::Path;
use std::process::Command;

/// Lines returned by one read when the caller does not ask for fewer.
const DEFAULT_READ_LINES: usize = 2_000;
/// The most context lines around a search match.
const MAX_CONTEXT: usize = 5;
const DEFAULT_MATCHES: usize = 100;
const MAX_MATCHES: usize = 200;
/// The most commits `git_log` returns.
const MAX_LOG_ENTRIES: usize = 50;

/// Whether `text` matches the glob `pattern`. `*` stays inside one path segment, `**` crosses
/// segments, `?` is one character. A pattern without a slash matches the file name alone, so
/// `*.rs` finds Rust files at any depth.
pub(super) fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    let target = if pattern.contains('/') {
        text
    } else {
        text.rsplit('/').next().unwrap_or(text)
    };
    let pattern_parts = pattern.split('/').collect::<Vec<_>>();
    let text_parts = target.split('/').collect::<Vec<_>>();
    match_segments(&pattern_parts, &text_parts)
}

fn match_segments(pattern: &[&str], text: &[&str]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((&"**", rest)) => {
            (0..=text.len()).any(|skipped| match_segments(rest, &text[skipped..]))
        }
        Some((segment, rest)) => match text.split_first() {
            Some((part, remaining)) => {
                match_segment(segment.as_bytes(), part.as_bytes())
                    && match_segments(rest, remaining)
            }
            None => false,
        },
    }
}

fn match_segment(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skipped| match_segment(rest, &text[skipped..])),
        Some((b'?', rest)) => !text.is_empty() && match_segment(rest, &text[1..]),
        Some((byte, rest)) => {
            text.first()
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(byte) || candidate == byte)
                && match_segment(rest, &text[1..])
        }
    }
}

/// Workspace-relative paths of the visible files, optionally limited to a folder and a glob.
fn visible_files(root: &Path, folder: Option<&str>, glob: Option<&str>) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut visited = 0;
    collect_files(root, root, &mut files, &mut visited, 0)?;
    files.sort();
    if let Some(folder) = folder
        .map(|f| f.trim_matches('/'))
        .filter(|f| !f.is_empty())
    {
        validate_relative_path(folder)?;
        let directory = root
            .join(folder)
            .canonicalize()
            .with_context(|| format!("resolving {folder}"))?;
        if !directory.starts_with(root) || !directory.is_dir() {
            bail!("{folder} is not a folder inside the workspace");
        }
        let prefix = format!("{}/", folder.replace('\\', "/"));
        files.retain(|file| file.starts_with(&prefix));
    }
    if let Some(glob) = glob.filter(|glob| !glob.trim().is_empty()) {
        files.retain(|file| glob_matches(glob, file));
    }
    Ok(files)
}

/// `list_files`: the visible files, optionally under one folder and matching a glob.
pub(super) fn list_files_in(
    root: &Path,
    folder: Option<&str>,
    glob: Option<&str>,
) -> Result<String> {
    let root = canonical_root(root)?;
    let mut files = visible_files(&root, folder, glob)?;
    let total = files.len();
    files.truncate(MAX_FILES);
    if files.is_empty() {
        return Ok("No files match.".to_owned());
    }
    let mut result = files.join("\n");
    if total > MAX_FILES {
        result.push_str(&format!(
            "\n… showing {MAX_FILES} of {total} files; narrow it with `path` or `pattern`"
        ));
    }
    Ok(result)
}

/// `read_file`: lines `offset..offset+limit` (one-based), each prefixed with its line number.
/// The number prefix is not part of the file.
pub(super) fn read_file_range(
    root: &Path,
    requested: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String> {
    let root = canonical_root(root)?;
    let path = super::resolve_path(&root, requested)?;
    let metadata = std::fs::metadata(&path).with_context(|| format!("reading {requested}"))?;
    if !metadata.is_file() {
        bail!("{requested} is not a regular file");
    }
    if metadata.len() > MAX_READ_BYTES {
        bail!(
            "{requested} exceeds the 512 KiB read limit; read a smaller file or search it instead"
        );
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("{requested} is not a UTF-8 text file"))?;
    let lines = text.lines().collect::<Vec<_>>();
    let total = lines.len();
    let start = offset.unwrap_or(1).max(1);
    let wanted = limit
        .unwrap_or(DEFAULT_READ_LINES)
        .clamp(1, DEFAULT_READ_LINES);
    if total == 0 {
        return Ok(format!("--- {requested} (empty file) ---"));
    }
    if start > total {
        bail!("{requested} has only {total} lines; offset {start} is past the end");
    }
    let end = (start + wanted - 1).min(total);
    let mut output = format!("--- {requested} (lines {start}-{end} of {total}) ---\n");
    for (index, line) in lines[start - 1..end].iter().enumerate() {
        output.push_str(&format!("{:>6}\t{line}\n", start + index));
    }
    if end < total {
        output.push_str(&format!(
            "… {} more lines; read on with offset={}\n",
            total - end,
            end + 1
        ));
    }
    Ok(output)
}

/// What `search_text` was asked for.
pub(super) struct SearchOptions<'a> {
    pub(super) query: &'a str,
    pub(super) regex: bool,
    pub(super) case_sensitive: bool,
    pub(super) context: usize,
    pub(super) folder: Option<&'a str>,
    pub(super) glob: Option<&'a str>,
    pub(super) max_results: usize,
}

impl<'a> SearchOptions<'a> {
    pub(super) fn literal(query: &'a str) -> SearchOptions<'a> {
        SearchOptions {
            query,
            regex: false,
            case_sensitive: false,
            context: 0,
            folder: None,
            glob: None,
            max_results: DEFAULT_MATCHES,
        }
    }
}

/// `search_text`: matching lines as `path:line: text`, with optional context lines
/// (`path-line- text`) and `--` between separate groups.
pub(super) fn search_with(root: &Path, options: &SearchOptions<'_>) -> Result<String> {
    if options.query.trim().is_empty() {
        bail!("search text cannot be empty");
    }
    let root = canonical_root(root)?;
    let pattern = if options.regex {
        options.query.to_owned()
    } else {
        regex::escape(options.query)
    };
    let matcher = RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|error| anyhow::anyhow!("invalid search pattern: {error}"))?;
    let context = options.context.min(MAX_CONTEXT);
    let limit = options.max_results.clamp(1, MAX_MATCHES);
    let mut output = String::new();
    let mut matches = 0usize;
    let mut files_with_matches = 0usize;
    let mut stopped_early = false;
    'files: for relative in visible_files(&root, options.folder, options.glob)? {
        let path = root.join(&relative);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_READ_BYTES {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines = contents.lines().collect::<Vec<_>>();
        // Index of the next line of this file that has not been printed yet.
        let mut printed_up_to = 0usize;
        let mut counted_file = false;
        for (index, line) in lines.iter().enumerate() {
            if !matcher.is_match(line) {
                continue;
            }
            if matches >= limit || output.len() >= MAX_OUTPUT_BYTES {
                stopped_early = true;
                break 'files;
            }
            if !counted_file {
                counted_file = true;
                files_with_matches += 1;
            }
            let from = index.saturating_sub(context).max(printed_up_to);
            let starts_new_group = printed_up_to == 0 || from > printed_up_to;
            if context > 0 && starts_new_group && !output.is_empty() {
                output.push_str("--\n");
            }
            for (number, before) in lines.iter().enumerate().take(index).skip(from) {
                output.push_str(&format!(
                    "{relative}-{}- {}\n",
                    number + 1,
                    before.trim_end()
                ));
            }
            output.push_str(&format!("{relative}:{}: {}\n", index + 1, line.trim_end()));
            printed_up_to = index + 1;
            // Context after a match stops at the next match, which prints its own lines.
            let last = (index + context).min(lines.len().saturating_sub(1));
            for (number, after) in lines.iter().enumerate().take(last + 1).skip(index + 1) {
                if matcher.is_match(after) {
                    break;
                }
                output.push_str(&format!(
                    "{relative}-{}- {}\n",
                    number + 1,
                    after.trim_end()
                ));
                printed_up_to = number + 1;
            }
            matches += 1;
        }
    }
    if matches == 0 {
        return Ok("No matches.".to_owned());
    }
    output.push_str(&format!(
        "{matches} match(es) in {files_with_matches} file(s){}",
        if stopped_early {
            "; stopped at the result limit, so narrow the search with `path` or `glob`"
        } else {
            ""
        }
    ));
    Ok(output)
}

fn run_git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(["-c", "core.fsmonitor=false", "--no-pager"])
        .args(args)
        .current_dir(root)
        .output()
        .context("running a read-only git command (is Git installed?)")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned())
}

/// `git_diff`: unstaged changes, or staged ones, optionally for one path.
pub(super) fn git_diff(root: &Path, path: Option<&str>, staged: bool) -> Result<String> {
    let root = canonical_root(root)?;
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    if let Some(path) = path.filter(|path| !path.trim().is_empty()) {
        validate_relative_path(path)?;
        args.push("--");
        args.push(path);
    }
    let diff = run_git(&root, &args)?;
    Ok(if diff.is_empty() {
        if staged {
            "No staged changes.".to_owned()
        } else {
            "No unstaged changes.".to_owned()
        }
    } else {
        diff
    })
}

/// `git_log`: recent commits, one per line, optionally for one path.
pub(super) fn git_log(root: &Path, limit: Option<usize>, path: Option<&str>) -> Result<String> {
    let root = canonical_root(root)?;
    let count = limit.unwrap_or(15).clamp(1, MAX_LOG_ENTRIES).to_string();
    let mut args = vec![
        "log",
        "--no-color",
        "--oneline",
        "--decorate",
        "-n",
        count.as_str(),
    ];
    if let Some(path) = path.filter(|path| !path.trim().is_empty()) {
        validate_relative_path(path)?;
        args.push("--");
        args.push(path);
    }
    let log = run_git(&root, &args)?;
    Ok(if log.is_empty() {
        "No commits.".to_owned()
    } else {
        log
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace(files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "harness-readtools-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        for (name, content) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn globs_match_like_people_expect() {
        for (pattern, text, expected) in [
            ("*.rs", "main.rs", true),
            ("*.rs", "src/tui/mod.rs", true),
            ("*.rs", "README.md", false),
            ("src/*.rs", "src/main.rs", true),
            ("src/*.rs", "src/tui/mod.rs", false),
            ("src/**/*.rs", "src/tui/mod.rs", true),
            ("src/**/*.rs", "src/main.rs", true),
            ("**/mod.rs", "src/tui/mod.rs", true),
            ("src/**", "src/a/b/c.txt", true),
            ("?.md", "a.md", true),
            ("?.md", "ab.md", false),
            ("*.RS", "main.rs", true),
            ("./src/*.rs", "src/main.rs", true),
            ("docs/*", "src/docs/x", false),
        ] {
            assert_eq!(glob_matches(pattern, text), expected, "{pattern} vs {text}");
        }
    }

    #[test]
    fn listing_can_be_scoped_to_a_folder_and_a_glob() {
        let root = workspace(&[
            ("src/main.rs", ""),
            ("src/tui/mod.rs", ""),
            ("docs/guide.md", ""),
            ("README.md", ""),
        ]);
        let all = list_files_in(&root, None, None).unwrap();
        assert_eq!(all.lines().count(), 4);
        let src = list_files_in(&root, Some("src"), None).unwrap();
        assert_eq!(src, "src/main.rs\nsrc/tui/mod.rs");
        let markdown = list_files_in(&root, None, Some("*.md")).unwrap();
        assert_eq!(markdown, "README.md\ndocs/guide.md");
        let both = list_files_in(&root, Some("src"), Some("*.rs")).unwrap();
        assert_eq!(both, "src/main.rs\nsrc/tui/mod.rs");
        assert_eq!(
            list_files_in(&root, None, Some("*.zip")).unwrap(),
            "No files match."
        );
        assert!(list_files_in(&root, Some("nope"), None).is_err());
        assert!(list_files_in(&root, Some("../elsewhere"), None).is_err());
    }

    #[test]
    fn reads_are_numbered_and_can_be_windowed() {
        let body = (1..=10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let root = workspace(&[("a.txt", &body)]);
        let whole = read_file_range(&root, "a.txt", None, None).unwrap();
        assert!(
            whole.starts_with("--- a.txt (lines 1-10 of 10) ---"),
            "{whole}"
        );
        assert!(whole.contains("     1\tline 1"), "{whole}");
        assert!(whole.contains("    10\tline 10"), "{whole}");
        let window = read_file_range(&root, "a.txt", Some(4), Some(3)).unwrap();
        assert!(window.contains("(lines 4-6 of 10)"), "{window}");
        assert!(window.contains("     4\tline 4") && window.contains("     6\tline 6"));
        assert!(!window.contains("line 3") && !window.contains("line 7"));
        assert!(
            window.contains("4 more lines; read on with offset=7"),
            "{window}"
        );
    }

    #[test]
    fn read_errors_say_what_is_wrong() {
        let root = workspace(&[("empty.txt", ""), ("small.txt", "one\ntwo")]);
        assert!(
            read_file_range(&root, "empty.txt", None, None)
                .unwrap()
                .contains("empty file")
        );
        let past = read_file_range(&root, "small.txt", Some(9), None).unwrap_err();
        assert!(format!("{past:#}").contains("only 2 lines"), "{past:#}");
        assert!(read_file_range(&root, "missing.txt", None, None).is_err());
        assert!(read_file_range(&root, "../x", None, None).is_err());
        assert!(
            read_file_range(&root, ".", None, None).is_err(),
            "a folder is not a file"
        );
    }

    #[test]
    fn a_range_read_matches_the_lines_edits_address() {
        let root = workspace(&[("a.txt", "alpha\nbeta\ngamma\n")]);
        let read = read_file_range(&root, "a.txt", Some(2), Some(1)).unwrap();
        assert!(read.contains("     2\tbeta"), "{read}");
    }

    #[test]
    fn search_is_literal_and_case_insensitive_by_default() {
        let root = workspace(&[
            ("a.rs", "fn Main() {}\nlet x = (1+2);\n"),
            ("b.md", "main thing\n"),
        ]);
        let found = search_with(&root, &SearchOptions::literal("MAIN")).unwrap();
        assert!(found.contains("a.rs:1: fn Main() {}"), "{found}");
        assert!(found.contains("b.md:1: main thing"), "{found}");
        assert!(found.ends_with("2 match(es) in 2 file(s)"), "{found}");
        let literal = search_with(&root, &SearchOptions::literal("(1+2)")).unwrap();
        assert!(
            literal.contains("a.rs:2:"),
            "special characters are literal: {literal}"
        );
        assert_eq!(
            search_with(&root, &SearchOptions::literal("zzz")).unwrap(),
            "No matches."
        );
        assert!(search_with(&root, &SearchOptions::literal("  ")).is_err());
    }

    #[test]
    fn search_can_use_regex_case_and_scopes() {
        let root = workspace(&[
            ("src/a.rs", "let value = 12;\nlet Value = 7;\n"),
            ("docs/b.md", "value 99\n"),
        ]);
        let regex = SearchOptions {
            regex: true,
            ..SearchOptions::literal(r"value = \d+")
        };
        let found = search_with(&root, &regex).unwrap();
        assert!(
            found.contains("src/a.rs:1:") && found.contains("src/a.rs:2:"),
            "{found}"
        );
        let sensitive = SearchOptions {
            case_sensitive: true,
            ..SearchOptions::literal("Value")
        };
        let only = search_with(&root, &sensitive).unwrap();
        assert!(
            only.contains("src/a.rs:2:") && !only.contains("src/a.rs:1:"),
            "{only}"
        );
        let scoped = SearchOptions {
            folder: Some("docs"),
            ..SearchOptions::literal("value")
        };
        let docs = search_with(&root, &scoped).unwrap();
        assert!(
            docs.contains("docs/b.md:1:") && !docs.contains("src/a.rs"),
            "{docs}"
        );
        let globbed = SearchOptions {
            glob: Some("*.rs"),
            ..SearchOptions::literal("value")
        };
        assert!(!search_with(&root, &globbed).unwrap().contains("docs/b.md"));
        let bad = SearchOptions {
            regex: true,
            ..SearchOptions::literal("(unclosed")
        };
        let error = search_with(&root, &bad).unwrap_err();
        assert!(
            format!("{error:#}").contains("invalid search pattern"),
            "{error:#}"
        );
    }

    #[test]
    fn context_lines_surround_matches_without_repeating() {
        let root = workspace(&[(
            "a.txt",
            "one\ntwo\nNEEDLE\nfour\nfive\nsix\nseven\nNEEDLE\nten\n",
        )]);
        let options = SearchOptions {
            context: 1,
            ..SearchOptions::literal("needle")
        };
        let found = search_with(&root, &options).unwrap();
        assert!(
            found.contains("a.txt-2- two\na.txt:3: NEEDLE\na.txt-4- four"),
            "{found}"
        );
        assert!(
            found.contains("--\n"),
            "separate groups are divided: {found}"
        );
        assert!(
            found.contains("a.txt-7- seven\na.txt:8: NEEDLE\na.txt-9- ten"),
            "{found}"
        );
        assert_eq!(
            found.matches("a.txt-4- four").count(),
            1,
            "no repeats: {found}"
        );
        assert!(found.ends_with("2 match(es) in 1 file(s)"));
    }

    #[test]
    fn adjacent_matches_share_their_context() {
        let root = workspace(&[("a.txt", "a\nNEEDLE\nNEEDLE\nz\n")]);
        let options = SearchOptions {
            context: 2,
            ..SearchOptions::literal("needle")
        };
        let found = search_with(&root, &options).unwrap();
        assert_eq!(found.matches("a.txt:2:").count(), 1);
        assert_eq!(found.matches("a.txt:3:").count(), 1);
        assert!(!found.contains("--\n"), "one continuous group: {found}");
    }

    #[test]
    fn many_matches_stop_at_the_limit_and_say_so() {
        let body = "hit\n".repeat(50);
        let root = workspace(&[("a.txt", &body)]);
        let options = SearchOptions {
            max_results: 5,
            ..SearchOptions::literal("hit")
        };
        let found = search_with(&root, &options).unwrap();
        assert_eq!(found.matches("a.txt:").count(), 5);
        assert!(found.contains("stopped at the result limit"), "{found}");
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn repo() -> PathBuf {
        let root = workspace(&[("a.txt", "one\n")]);
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&root)
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first commit"]);
        root
    }

    #[test]
    fn git_diff_shows_unstaged_and_staged_changes() {
        if !git_available() {
            return;
        }
        let root = repo();
        assert_eq!(
            git_diff(&root, None, false).unwrap(),
            "No unstaged changes."
        );
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let unstaged = git_diff(&root, None, false).unwrap();
        assert!(unstaged.contains("+two"), "{unstaged}");
        assert_eq!(git_diff(&root, None, true).unwrap(), "No staged changes.");
        Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(git_diff(&root, None, true).unwrap().contains("+two"));
        assert!(
            git_diff(&root, Some("a.txt"), true)
                .unwrap()
                .contains("+two")
        );
        assert!(git_diff(&root, Some("../escape"), false).is_err());
    }

    #[test]
    fn git_log_lists_recent_commits() {
        if !git_available() {
            return;
        }
        let root = repo();
        let log = git_log(&root, None, None).unwrap();
        assert!(log.contains("first commit"), "{log}");
        assert_eq!(
            git_log(&root, Some(1), Some("a.txt"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(
            git_log(&root, Some(9999), None).is_ok(),
            "the limit is clamped"
        );
        assert!(git_log(&root, None, Some("/etc/passwd")).is_err());
    }

    #[test]
    fn git_commands_outside_a_repository_explain_themselves() {
        if !git_available() {
            return;
        }
        let root = workspace(&[("a.txt", "x")]);
        let error = git_diff(&root, None, false);
        if let Err(error) = error {
            assert!(
                format!("{error:#}").contains("git diff failed"),
                "{error:#}"
            );
        }
    }
}
