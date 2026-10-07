use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

mod readtools;

#[derive(Clone, Debug)]
pub(crate) struct ToolDefinition {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) parameters: Value,
}

pub(crate) fn definitions() -> [ToolDefinition; 12] {
    use serde_json::json;
    [
        ToolDefinition {
            name: "list_files",
            description: "List files in the workspace (hidden and generated folders such as .git, target and node_modules are skipped). Start broad, then narrow: `path` looks inside one folder and `pattern` is a glob such as `*.rs` or `src/**/*.ts`.",
            parameters: json!({"type":"object", "properties":{
                "path":{"type":"string", "description":"Folder to list, relative to the workspace root."},
                "pattern":{"type":"string", "description":"Glob the file path must match. Without a slash it matches file names at any depth."}
            }, "additionalProperties":false}),
        },
        ToolDefinition {
            name: "read_file",
            description: "Read a text file inside the workspace. Every output line starts with its line number and a tab; that prefix is not part of the file. Large files are returned in pieces: use `offset` and `limit` to read just the part you need. Read a file before you edit it.",
            parameters: json!({"type":"object", "properties":{
                "path":{"type":"string", "description":"Workspace-relative path."},
                "offset":{"type":"integer", "minimum":1, "description":"First line to return (one-based). Default 1."},
                "limit":{"type":"integer", "minimum":1, "description":"Number of lines to return. Default and maximum 2000."}
            }, "required":["path"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "search_text",
            description: "Search file contents. The query is a literal, case-insensitive phrase unless you set `regex` (regular expression) or `case_sensitive`. Narrow with `path` (a folder) and `glob`; `context` adds lines around each match. Results look like `file:line: text`.",
            parameters: json!({"type":"object", "properties":{
                "query":{"type":"string", "description":"What to look for."},
                "regex":{"type":"boolean", "description":"Treat the query as a regular expression."},
                "case_sensitive":{"type":"boolean", "description":"Match letter case exactly."},
                "context":{"type":"integer", "minimum":0, "maximum":5, "description":"Lines of context before and after each match."},
                "path":{"type":"string", "description":"Only search under this folder."},
                "glob":{"type":"string", "description":"Only search files whose path matches this glob, e.g. `*.rs`."},
                "max_results":{"type":"integer", "minimum":1, "maximum":200, "description":"Stop after this many matching lines. Default 100."}
            }, "required":["query"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "git_status",
            description: "Read-only `git status --short --branch`: the current branch and which files are modified, added or untracked.",
            parameters: json!({"type":"object", "properties":{}, "additionalProperties":false}),
        },
        ToolDefinition {
            name: "git_diff",
            description: "Read-only diff of uncommitted changes (or of the staged changes with `staged`), optionally for one path. Use it to review your own edits before reporting them.",
            parameters: json!({"type":"object", "properties":{
                "path":{"type":"string", "description":"Limit the diff to this file or folder."},
                "staged":{"type":"boolean", "description":"Show staged changes instead of unstaged ones."}
            }, "additionalProperties":false}),
        },
        ToolDefinition {
            name: "git_log",
            description: "Read-only list of recent commits, one per line, optionally for one path. Useful for the project's commit style and for finding when something changed.",
            parameters: json!({"type":"object", "properties":{
                "limit":{"type":"integer", "minimum":1, "maximum":50, "description":"Number of commits. Default 15."},
                "path":{"type":"string", "description":"Only commits touching this file or folder."}
            }, "additionalProperties":false}),
        },
        ToolDefinition {
            name: "replace_text",
            description: "Edit an existing file by replacing exact text. `old_text` must match the file exactly, including indentation, and must be unique in the file unless `replace_all` is true; include a few neighbouring lines to make it unique. This is the preferred way to change existing code. The harness shows the change and may ask the user to approve it.",
            parameters: json!({"type":"object", "properties":{
                "path":{"type":"string", "description":"Workspace-relative path of an existing file."},
                "old_text":{"type":"string", "description":"The exact text to replace (no line-number prefixes)."},
                "new_text":{"type":"string", "description":"The text to put in its place."},
                "replace_all":{"type":"boolean", "description":"Replace every occurrence instead of requiring a unique match."}
            }, "required":["path","old_text","new_text"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "replace_in_file",
            description: "Replace an inclusive, one-based line range. Use it when replace_text cannot express the change (for example replacing a whole block by position). Copy `expected_text` from the file without the line-number prefixes so the harness can detect stale context.",
            parameters: json!({"type":"object", "properties":{
                "path":{"type":"string"},
                "start_line":{"type":"integer", "minimum":1},
                "end_line":{"type":"integer", "minimum":1},
                "expected_text":{"type":"string", "description":"The current text of those lines, exactly."},
                "replacement":{"type":"string", "description":"What the lines become (may be empty to delete them)."}
            }, "required":["path","start_line","end_line","expected_text","replacement"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "write_to_file",
            description: "Insert text before a given line of an existing file (one-based; use line_count + 1 to append, and 0 only for an empty file). To change existing text use replace_text instead.",
            parameters: json!({"type":"object", "properties":{
                "path":{"type":"string"},
                "line_number":{"type":"integer", "minimum":0},
                "text":{"type":"string"}
            }, "required":["path","line_number","text"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "create_file",
            description: "Create a new file with its complete contents. Fails rather than overwriting an existing file, and the parent folder must already exist.",
            parameters: json!({"type":"object", "properties":{
                "path":{"type":"string"},
                "content":{"type":"string"}
            }, "required":["path","content"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "run_command",
            description: "Run a shell command in the workspace root (PowerShell on Windows, sh elsewhere) to build, test, lint or inspect. The command must not wait for input. Output is truncated and a command is stopped after five minutes. The harness applies the permission mode, so the user may be asked first and may decline.",
            parameters: json!({"type":"object", "properties":{
                "command":{"type":"string", "description":"The exact command line."}
            }, "required":["command"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "request_plan_approval",
            description: "In Plan mode, present an exact ordered list of edits, file creations and commands for the user to approve before performing any of them. Afterwards perform only those actions, with exactly those arguments, in that order.",
            parameters: json!({"type":"object", "properties":{
                "summary":{"type":"string", "description":"One or two sentences on what the plan achieves."},
                "actions":{"type":"array", "items":{"type":"object", "properties":{
                    "name":{"type":"string", "enum":["replace_text", "replace_in_file", "write_to_file", "create_file", "run_command"]},
                    "arguments":{"type":"object"}
                }, "required":["name", "arguments"], "additionalProperties":false}}
            }, "required":["summary", "actions"], "additionalProperties":false}),
        },
    ]
}

/// Which tools a request offers the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolSet {
    /// No tools: a plain chat turn.
    None,
    /// The main assistant: every tool, plus `spawn_subagents` while workflows are on.
    Main { plan_mode: bool, workflows: bool },
    /// A subagent that may only look: no edits, no commands.
    Explore,
    /// A subagent that may also edit files and run commands (with the usual approvals).
    Implement,
}

/// The names of the tools that only read the repository.
pub(crate) const READ_ONLY_TOOLS: [&str; 6] = [
    "list_files",
    "read_file",
    "search_text",
    "git_status",
    "git_diff",
    "git_log",
];

impl ToolSet {
    pub(crate) fn any(self) -> bool {
        self != ToolSet::None
    }

    pub(crate) fn definitions(self) -> Vec<ToolDefinition> {
        match self {
            ToolSet::None => Vec::new(),
            ToolSet::Main {
                plan_mode,
                workflows,
            } => {
                let mut tools = definitions_for_mode(if plan_mode { "plan" } else { "auto" });
                if workflows {
                    tools.push(spawn_subagents_definition());
                }
                tools
            }
            ToolSet::Explore => definitions()
                .into_iter()
                .filter(|tool| READ_ONLY_TOOLS.contains(&tool.name))
                .collect(),
            ToolSet::Implement => definitions()
                .into_iter()
                .filter(|tool| tool.name != "request_plan_approval")
                .collect(),
        }
    }

    /// Whether the tool may be called by an agent that was offered this set.
    pub(crate) fn allows(self, name: &str) -> bool {
        self.definitions().iter().any(|tool| tool.name == name)
    }
}

/// The tool the main assistant uses to hand work to subagents (offered only while workflows
/// are on).
pub(crate) fn spawn_subagents_definition() -> ToolDefinition {
    ToolDefinition {
        name: "spawn_subagents",
        description: "Hand independent pieces of work to subagents and get their reports back. Subagents do not see this conversation, so give each one complete instructions. `explore` subagents only read and search the repository and run in parallel; `implement` subagents can also edit files and run commands (with the usual approvals) and run one after another. Use this for research across several areas or for clearly separate changes; do small tasks yourself.",
        parameters: serde_json::json!({"type":"object", "properties":{
            "tasks":{"type":"array", "minItems":1, "items":{"type":"object", "properties":{
                "name":{"type":"string", "description":"A short label for the report, e.g. `auth flow`."},
                "kind":{"type":"string", "enum":["explore","implement"]},
                "instructions":{"type":"string", "description":"Everything the subagent needs: the goal, the files or areas to look at, constraints, and what to report back."}
            }, "required":["kind","instructions"], "additionalProperties":false}}
        }, "required":["tasks"], "additionalProperties":false}),
    }
}

pub(crate) fn definitions_for_mode(permission_mode: &str) -> Vec<ToolDefinition> {
    definitions()
        .into_iter()
        .filter(|tool| permission_mode == "plan" || tool.name != "request_plan_approval")
        .collect()
}

#[derive(Debug)]
pub(crate) struct EditProposal {
    pub(crate) relative_path: String,
    pub(crate) original: String,
    pub(crate) updated: String,
    pub(crate) change_summary: String,
    pub(crate) before: String,
    pub(crate) after: String,
}

impl EditProposal {
    pub(crate) fn is_small(&self) -> bool {
        self.before.len() <= 2_048 && self.after.len() <= 2_048
    }

    pub(crate) fn preview(&self) -> String {
        format!(
            "File: {}\n{}\n\n- {}\n+ {}",
            self.relative_path,
            self.change_summary,
            self.before.replace('\n', "\n- "),
            self.after.replace('\n', "\n+ "),
        )
    }
}

pub(crate) struct CreateProposal {
    pub(crate) relative_path: String,
    pub(crate) content: String,
}

impl CreateProposal {
    pub(crate) fn preview(&self) -> String {
        format!("New file: {}\n\n{}", self.relative_path, self.content)
    }

    pub(crate) fn is_small(&self) -> bool {
        self.content.len() <= 2_048
    }
}

pub(crate) fn prepare_replace_lines(
    root: &Path,
    requested: &str,
    start_line: usize,
    end_line: usize,
    expected_text: &str,
    replacement: &str,
) -> Result<EditProposal> {
    if start_line == 0 || end_line < start_line {
        bail!("replacement lines must be a one-based inclusive range");
    }
    if expected_text.len() > 8 * 1024 || replacement.len() > 8 * 1024 {
        bail!("expected and replacement text must be no more than 8 KiB each");
    }
    let root = canonical_root(root)?;
    let path = resolve_path(&root, requested)?;
    let metadata =
        fs::metadata(&path).with_context(|| format!("reading {} metadata", requested))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        bail!("edits are limited to existing regular text files up to 1 MiB");
    }
    let original = fs::read_to_string(&path).with_context(|| format!("reading {requested}"))?;
    let ranges = line_ranges(&original);
    if end_line > ranges.len() {
        bail!(
            "line range {start_line}-{end_line} is outside {requested}, which has {} lines",
            ranges.len()
        );
    }
    let range_start = ranges[start_line - 1].0;
    let range_end = ranges[end_line - 1].1;
    let before = original[range_start..range_end].to_owned();
    if normalize_newlines(&before).trim_end_matches('\n')
        != normalize_newlines(expected_text).trim_end_matches('\n')
    {
        bail!(
            "lines {start_line}-{end_line} of {requested} no longer match expected_text; re-read them before editing"
        );
    }
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut replacement = normalize_newlines(replacement).replace('\n', newline);
    if before.ends_with('\n') && !replacement.is_empty() && !replacement.ends_with(newline) {
        replacement.push_str(newline);
    }
    let mut updated = String::with_capacity(original.len() + replacement.len());
    updated.push_str(&original[..range_start]);
    updated.push_str(&replacement);
    updated.push_str(&original[range_end..]);
    if updated.len() > 1024 * 1024 {
        bail!("the updated file would exceed the 1 MiB edit limit");
    }
    Ok(EditProposal {
        relative_path: requested.to_owned(),
        original,
        updated,
        change_summary: format!("Replace lines {start_line}-{end_line}"),
        before: before.trim_end_matches(['\r', '\n']).to_owned(),
        after: replacement.to_owned(),
    })
}

/// Replaces exact text in an existing file. `old_text` must occur exactly once unless
/// `replace_all` is set. Line endings are matched loosely (the file's own style is kept).
pub(crate) fn prepare_replace_text(
    root: &Path,
    requested: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<EditProposal> {
    if old_text.is_empty() {
        bail!("old_text must not be empty; use write_to_file or create_file to add text");
    }
    if old_text == new_text {
        bail!("old_text and new_text are identical; there is nothing to change");
    }
    if old_text.len() > 64 * 1024 || new_text.len() > 64 * 1024 {
        bail!("old_text and new_text must be no more than 64 KiB each");
    }
    let root = canonical_root(root)?;
    let path = resolve_path(&root, requested)?;
    let metadata = fs::metadata(&path).with_context(|| format!("reading {requested} metadata"))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        bail!("edits are limited to existing regular text files up to 1 MiB");
    }
    let original = fs::read_to_string(&path).with_context(|| format!("reading {requested}"))?;
    let crlf = original.contains("\r\n");
    let normalized = normalize_newlines(&original);
    let old = normalize_newlines(old_text);
    let new = normalize_newlines(new_text);
    let occurrences = normalized.matches(&old).count();
    if occurrences == 0 {
        bail!(
            "old_text was not found in {requested}. Check the indentation and line breaks, and read the file again: it may have changed."
        );
    }
    if occurrences > 1 && !replace_all {
        bail!(
            "old_text appears {occurrences} times in {requested}. Add neighbouring lines to make it unique, or set replace_all to true to change every occurrence."
        );
    }
    let first = normalized.find(&old).unwrap_or(0);
    let line = normalized[..first].matches('\n').count() + 1;
    let changed = if replace_all {
        normalized.replace(&old, &new)
    } else {
        normalized.replacen(&old, &new, 1)
    };
    let updated = if crlf {
        changed.replace('\n', "\r\n")
    } else {
        changed
    };
    if updated.len() > 1024 * 1024 {
        bail!("the updated file would exceed the 1 MiB edit limit");
    }
    let replaced = if replace_all { occurrences } else { 1 };
    Ok(EditProposal {
        relative_path: requested.to_owned(),
        original,
        updated,
        change_summary: format!(
            "Replace {replaced} occurrence{} (first at line {line})",
            if replaced == 1 { "" } else { "s" }
        ),
        before: old.trim_end_matches('\n').to_owned(),
        after: new.trim_end_matches('\n').to_owned(),
    })
}

pub(crate) fn prepare_write_to_file(
    root: &Path,
    requested: &str,
    line_number: usize,
    text: &str,
) -> Result<EditProposal> {
    if text.is_empty() || text.len() > 64 * 1024 {
        bail!("write_to_file text must be non-empty and no more than 64 KiB");
    }
    let root = canonical_root(root)?;
    let path = resolve_path(&root, requested)?;
    let metadata = fs::metadata(&path).with_context(|| format!("reading {requested} metadata"))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        bail!("write_to_file supports existing regular text files up to 1 MiB");
    }
    let original = fs::read_to_string(&path).with_context(|| format!("reading {requested}"))?;
    let ranges = line_ranges(&original);
    let offset = if original.is_empty() {
        if line_number != 0 {
            bail!("use line_number 0 when writing into an empty file");
        }
        0
    } else {
        if line_number == 0 || line_number > ranges.len() + 1 {
            bail!(
                "line_number must be between 1 and {} for this file",
                ranges.len() + 1
            );
        }
        if line_number == ranges.len() + 1 {
            original.len()
        } else {
            ranges[line_number - 1].0
        }
    };
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut insertion = normalize_newlines(text).replace('\n', newline);
    if !original.is_empty() && offset == original.len() && !original.ends_with('\n') {
        insertion.insert_str(0, newline);
    }
    if offset < original.len() && !insertion.ends_with(newline) {
        insertion.push_str(newline);
    }
    let mut updated = String::with_capacity(original.len() + insertion.len());
    updated.push_str(&original[..offset]);
    updated.push_str(&insertion);
    updated.push_str(&original[offset..]);
    if updated.len() > 1024 * 1024 {
        bail!("the updated file would exceed the 1 MiB edit limit");
    }
    Ok(EditProposal {
        relative_path: requested.to_owned(),
        original,
        updated,
        change_summary: format!("Insert at line {line_number}"),
        before: String::new(),
        after: text.to_owned(),
    })
}

pub(crate) fn prepare_create_file(
    root: &Path,
    requested: &str,
    content: &str,
) -> Result<CreateProposal> {
    if content.len() > 64 * 1024 {
        bail!("new file content is limited to 64 KiB so it can be reviewed before creation");
    }
    let root = canonical_root(root)?;
    let path = resolve_new_path(&root, requested)?;
    if path.exists() {
        bail!(
            "{} already exists; create_file will never overwrite it",
            requested
        );
    }
    Ok(CreateProposal {
        relative_path: requested.to_owned(),
        content: content.to_owned(),
    })
}

pub(crate) fn apply_edit(root: &Path, proposal: &EditProposal) -> Result<()> {
    let root = canonical_root(root)?;
    let path = resolve_path(&root, &proposal.relative_path)?;
    let current = fs::read_to_string(&path)
        .with_context(|| format!("rechecking {} before edit", proposal.relative_path))?;
    if current != proposal.original {
        bail!(
            "{} changed after the edit was prepared; no edit was applied",
            proposal.relative_path
        );
    }
    fs::write(&path, &proposal.updated)
        .with_context(|| format!("writing edited file {}", proposal.relative_path))
}

pub(crate) fn apply_create(root: &Path, proposal: &CreateProposal) -> Result<()> {
    let root = canonical_root(root)?;
    let path = resolve_new_path(&root, &proposal.relative_path)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("creating {} without overwriting", proposal.relative_path))?;
    use std::io::Write as _;
    file.write_all(proposal.content.as_bytes())
        .with_context(|| format!("writing initial contents for {}", proposal.relative_path))
}

fn line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let end = offset + line.len();
        ranges.push((offset, end));
        offset = end;
    }
    if !text.is_empty() && !text.ends_with('\n') && ranges.is_empty() {
        ranges.push((0, text.len()));
    }
    ranges
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) fn run_command(
    root: &Path,
    command: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String> {
    let command = command.trim();
    if command.is_empty() || command.len() > 8 * 1024 || command.contains('\0') {
        bail!("command must be non-empty and no longer than 8 KiB");
    }
    let root = canonical_root(root)?;
    let mut process = if cfg!(windows) {
        let mut process = Command::new("powershell.exe");
        process.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            command,
        ]);
        process
    } else {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        process
    };
    process
        .current_dir(&root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = process.spawn().context("starting approved shell command")?;
    let stdout = child.stdout.take().context("capturing command stdout")?;
    let stderr = child.stderr.take().context("capturing command stderr")?;
    let stdout_reader = thread::spawn(|| read_bounded(stdout));
    let stderr_reader = thread::spawn(|| read_bounded(stderr));
    let deadline = Instant::now() + Duration::from_secs(300);
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for command")? {
            break status;
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("command cancelled");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("approved command exceeded the 5-minute time limit");
        }
        thread::sleep(Duration::from_millis(100));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader stopped unexpectedly"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader stopped unexpectedly"))??;
    let mut result = format!(
        "Exit status: {}\n",
        status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated by signal".to_owned())
    );
    if !stdout.trim().is_empty() {
        result.push_str("stdout:\n");
        result.push_str(&stdout);
        result.push('\n');
    }
    if !stderr.trim().is_empty() {
        result.push_str("stderr:\n");
        result.push_str(&stderr);
        result.push('\n');
    }
    Ok(result.chars().take(MAX_OUTPUT_BYTES).collect())
}

fn read_bounded<R: Read>(mut reader: R) -> Result<String> {
    const LIMIT: usize = 24 * 1024;
    let mut kept = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = reader.read(&mut buffer).context("reading command output")?;
        if count == 0 {
            break;
        }
        let remaining = LIMIT.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    let mut output = String::from_utf8_lossy(&kept).into_owned();
    if kept.len() >= LIMIT {
        output.push_str("\n… output truncated");
    }
    Ok(output)
}

const MAX_READ_BYTES: u64 = 512 * 1024;
const MAX_FILES: usize = 500;
const MAX_DIRECTORY_ENTRIES: usize = 10_000;
const MAX_DIRECTORY_DEPTH: usize = 32;
const MAX_OUTPUT_BYTES: usize = 48 * 1024;

pub(crate) fn list_files(root: &Path) -> Result<String> {
    readtools::list_files_in(root, None, None)
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<String>,
    visited: &mut usize,
    depth: usize,
) -> Result<()> {
    if files.len() > MAX_FILES || *visited >= MAX_DIRECTORY_ENTRIES || depth >= MAX_DIRECTORY_DEPTH
    {
        return Ok(());
    }
    let entries =
        fs::read_dir(directory).with_context(|| format!("listing {}", directory.display()))?;
    for entry in entries {
        *visited += 1;
        if *visited > MAX_DIRECTORY_ENTRIES {
            break;
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if ignored_name(&name) {
            continue;
        }
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(root, &path, files, visited, depth + 1)?;
        } else if file_type.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

pub(crate) fn read_file(root: &Path, requested: &str) -> Result<String> {
    let root = canonical_root(root)?;
    let path = resolve_path(&root, requested)?;
    let metadata =
        fs::metadata(&path).with_context(|| format!("reading {} metadata", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a regular file", requested);
    }
    if metadata.len() > MAX_READ_BYTES {
        bail!("{} exceeds the 512 KiB read limit", requested);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("reading text file {}", requested))?;
    Ok(format!("--- {requested} ---\n{text}"))
}

pub(crate) fn search(root: &Path, query: &str) -> Result<String> {
    readtools::search_with(root, &readtools::SearchOptions::literal(query))
}

pub(crate) fn git_status(root: &Path) -> Result<String> {
    let root = canonical_root(root)?;
    let output = Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "status",
            "--short",
            "--branch",
        ])
        .current_dir(&root)
        .output()
        .context("running read-only `git status --short --branch` (is Git installed?)")?;
    if !output.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn execute_read_only(root: &Path, name: &str, arguments: &Value) -> Result<String> {
    let output = match name {
        "list_files" => {
            validate_arguments(arguments, &["path", "pattern"])?;
            readtools::list_files_in(
                root,
                optional_string(arguments, "path")?,
                optional_string(arguments, "pattern")?,
            )?
        }
        "read_file" => {
            validate_arguments(arguments, &["path", "offset", "limit"])?;
            readtools::read_file_range(
                root,
                required_string(arguments, "path")?,
                optional_count(arguments, "offset")?,
                optional_count(arguments, "limit")?,
            )?
        }
        "search_text" => {
            validate_arguments(
                arguments,
                &[
                    "query",
                    "regex",
                    "case_sensitive",
                    "context",
                    "path",
                    "glob",
                    "max_results",
                ],
            )?;
            readtools::search_with(
                root,
                &readtools::SearchOptions {
                    query: required_string(arguments, "query")?,
                    regex: optional_bool(arguments, "regex")?,
                    case_sensitive: optional_bool(arguments, "case_sensitive")?,
                    context: optional_count(arguments, "context")?.unwrap_or(0),
                    folder: optional_string(arguments, "path")?,
                    glob: optional_string(arguments, "glob")?,
                    max_results: optional_count(arguments, "max_results")?.unwrap_or(100),
                },
            )?
        }
        "git_status" => {
            validate_arguments(arguments, &[])?;
            git_status(root)?
        }
        "git_diff" => {
            validate_arguments(arguments, &["path", "staged"])?;
            readtools::git_diff(
                root,
                optional_string(arguments, "path")?,
                optional_bool(arguments, "staged")?,
            )?
        }
        "git_log" => {
            validate_arguments(arguments, &["limit", "path"])?;
            readtools::git_log(
                root,
                optional_count(arguments, "limit")?,
                optional_string(arguments, "path")?,
            )?
        }
        _ => bail!("unknown or unauthorized tool `{name}`"),
    };
    Ok(output.chars().take(MAX_OUTPUT_BYTES).collect())
}

/// An optional string argument; `None` when absent or null.
fn optional_string<'a>(arguments: &'a Value, name: &str) -> Result<Option<&'a str>> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => bail!("tool argument `{name}` must be a string"),
    }
}

/// An optional non-negative whole number argument.
fn optional_count(arguments: &Value, name: &str) -> Result<Option<usize>> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|number| usize::try_from(number).ok())
            .map(Some)
            .with_context(|| format!("tool argument `{name}` must be a non-negative whole number")),
    }
}

fn optional_bool(arguments: &Value, name: &str) -> Result<bool> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => bail!("tool argument `{name}` must be true or false"),
    }
}

fn validate_arguments(arguments: &Value, allowed: &[&str]) -> Result<()> {
    let object = arguments
        .as_object()
        .context("tool arguments must be a JSON object")?;
    if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("unknown argument `{unknown}`");
    }
    Ok(())
}

fn required_string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("tool argument `{name}` must be a non-empty string"))
}

fn resolve_path(root: &Path, requested: &str) -> Result<PathBuf> {
    let relative = validate_relative_path(requested)?;
    let path = root
        .join(relative)
        .canonicalize()
        .with_context(|| format!("resolving {requested}"))?;
    if !path.starts_with(root) {
        bail!("{requested} resolves outside the workspace");
    }
    Ok(path)
}

fn resolve_new_path(root: &Path, requested: &str) -> Result<PathBuf> {
    let relative = validate_relative_path(requested)?;
    let parent = relative.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = root
        .join(parent)
        .canonicalize()
        .with_context(|| format!("resolving parent directory for {requested}"))?;
    if !canonical_parent.starts_with(root) {
        bail!("new file path resolves outside the workspace");
    }
    let name = relative
        .file_name()
        .context("new file path must include a file name")?;
    let destination = canonical_parent.join(name);
    if destination.exists() {
        bail!("{} already exists", requested);
    }
    Ok(destination)
}

fn validate_relative_path(requested: &str) -> Result<&Path> {
    let relative = Path::new(requested);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("paths must be relative and stay inside the workspace");
    }
    Ok(relative)
}

fn canonical_root(root: &Path) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("resolving workspace {}", root.display()))
}

fn ignored_name(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "target" | "node_modules" | "vendor" | "dist" | "build"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> PathBuf {
        let root = std::env::temp_dir().join(format!("coolcode-tools-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create test workspace");
        root
    }

    #[test]
    fn reads_and_lists_only_workspace_files() {
        let root = workspace();
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("source file");
        fs::create_dir_all(root.join(".git")).expect("git dir");
        fs::write(root.join(".git/config"), "private metadata").expect("git metadata");
        assert!(list_files(&root).expect("list").contains("src/main.rs"));
        assert!(!list_files(&root).expect("list").contains(".git/config"));
        assert!(
            read_file(&root, "src/main.rs")
                .expect("read")
                .contains("fn main")
        );
        assert!(read_file(&root, "../secret").is_err());
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn replace_text_changes_one_unique_match_and_previews_it() {
        let root = workspace();
        fs::write(root.join("a.rs"), "fn one() {}\nfn two() {}\n").expect("file");
        let proposal =
            prepare_replace_text(&root, "a.rs", "fn two() {}", "fn two() -> u8 { 2 }", false)
                .expect("proposal");
        assert_eq!(proposal.updated, "fn one() {}\nfn two() -> u8 { 2 }\n");
        assert!(
            proposal.change_summary.contains("first at line 2"),
            "{}",
            proposal.change_summary
        );
        assert!(
            proposal.preview().contains("- fn two() {}"),
            "{}",
            proposal.preview()
        );
        apply_edit(&root, &proposal).expect("apply");
        assert_eq!(
            fs::read_to_string(root.join("a.rs")).unwrap(),
            "fn one() {}\nfn two() -> u8 { 2 }\n"
        );
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn replace_text_refuses_ambiguity_and_absence_with_actionable_messages() {
        let root = workspace();
        fs::write(root.join("a.txt"), "x = 1\ny = 1\n").expect("file");
        let ambiguous = prepare_replace_text(&root, "a.txt", "= 1", "= 2", false).unwrap_err();
        assert!(
            format!("{ambiguous:#}").contains("appears 2 times"),
            "{ambiguous:#}"
        );
        assert!(
            format!("{ambiguous:#}").contains("replace_all"),
            "{ambiguous:#}"
        );
        let missing = prepare_replace_text(&root, "a.txt", "z = 1", "z = 2", false).unwrap_err();
        assert!(format!("{missing:#}").contains("not found"), "{missing:#}");
        let all = prepare_replace_text(&root, "a.txt", "= 1", "= 2", true).expect("replace all");
        assert_eq!(all.updated, "x = 2\ny = 2\n");
        assert!(
            all.change_summary.contains("2 occurrences"),
            "{}",
            all.change_summary
        );
        assert!(prepare_replace_text(&root, "a.txt", "", "x", false).is_err());
        assert!(prepare_replace_text(&root, "a.txt", "x", "x", false).is_err());
        assert!(prepare_replace_text(&root, "../outside", "x", "y", false).is_err());
        assert!(prepare_replace_text(&root, "missing.txt", "x", "y", false).is_err());
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn replace_text_can_delete_and_keeps_windows_line_endings() {
        let root = workspace();
        fs::write(root.join("w.txt"), "a\r\nb\r\nc\r\n").expect("file");
        let deletion = prepare_replace_text(&root, "w.txt", "b\n", "", false).expect("delete");
        assert_eq!(
            deletion.updated, "a\r\nc\r\n",
            "matched across the file's CRLF, kept CRLF"
        );
        let edit = prepare_replace_text(&root, "w.txt", "a\nb", "A\nB", false).expect("edit");
        assert_eq!(edit.updated, "A\r\nB\r\nc\r\n");
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn a_stale_replace_text_proposal_is_not_applied() {
        let root = workspace();
        fs::write(root.join("a.txt"), "one\n").expect("file");
        let proposal = prepare_replace_text(&root, "a.txt", "one", "two", false).expect("proposal");
        fs::write(root.join("a.txt"), "changed meanwhile\n").expect("concurrent edit");
        assert!(apply_edit(&root, &proposal).is_err());
        assert_eq!(
            fs::read_to_string(root.join("a.txt")).unwrap(),
            "changed meanwhile\n"
        );
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn the_read_only_dispatcher_passes_options_through_and_checks_their_types() {
        let root = workspace();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "alpha\nbeta\ngamma\n").unwrap();
        fs::write(root.join("notes.md"), "beta in notes\n").unwrap();
        let run = |name: &str, arguments: Value| execute_read_only(&root, name, &arguments);
        let listing = run("list_files", serde_json::json!({"path": "src"})).unwrap();
        assert_eq!(listing, "src/a.rs");
        let read = run(
            "read_file",
            serde_json::json!({"path": "src/a.rs", "offset": 2, "limit": 1}),
        )
        .unwrap();
        assert!(
            read.contains("     2\tbeta") && !read.contains("gamma"),
            "{read}"
        );
        let search = run(
            "search_text",
            serde_json::json!({"query": "BETA", "glob": "*.rs", "context": 1}),
        )
        .unwrap();
        assert!(
            search.contains("src/a.rs:2: beta") && !search.contains("notes.md"),
            "{search}"
        );
        assert!(
            run(
                "read_file",
                serde_json::json!({"path": "src/a.rs", "offset": "two"})
            )
            .is_err()
        );
        assert!(
            run(
                "search_text",
                serde_json::json!({"query": "x", "regex": "yes"})
            )
            .is_err()
        );
        assert!(run("list_files", serde_json::json!({"path": 5})).is_err());
        assert!(run("list_files", serde_json::json!({"nonsense": true})).is_err());
        assert!(run("read_file", serde_json::json!({})).is_err());
        assert!(run("git_log", serde_json::json!({"limit": -1})).is_err());
        assert!(
            run("shell", serde_json::json!({})).is_err(),
            "unknown tools are refused"
        );
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn every_tool_has_a_description_and_documented_parameters() {
        for tool in definitions() {
            assert!(
                tool.description.len() > 40,
                "{} needs a real description",
                tool.name
            );
            let properties = tool.parameters["properties"]
                .as_object()
                .expect("properties");
            let required = tool.parameters["required"]
                .as_array()
                .map(|names| names.len())
                .unwrap_or(0);
            assert!(required <= properties.len(), "{}", tool.name);
            if tool.name != "request_plan_approval" {
                for (name, schema) in properties {
                    // Self-explanatory names may skip prose; anything optional must explain itself.
                    let documented = schema.get("description").is_some();
                    let obvious = [
                        "path",
                        "command",
                        "content",
                        "text",
                        "line_number",
                        "start_line",
                        "end_line",
                    ];
                    assert!(
                        documented || obvious.contains(&name.as_str()),
                        "{}.{name}",
                        tool.name
                    );
                }
            }
        }
        let names = definitions()
            .iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        let unique = names.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), names.len(), "tool names are unique");
    }

    #[test]
    fn text_search_returns_path_and_line_number() {
        let root = workspace();
        fs::write(root.join("notes.txt"), "first\nNeedle in a haystack\n").expect("file");
        let result = search(&root, "needle").expect("search");
        assert!(result.contains("notes.txt:2:"));
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn line_replacement_checks_the_requested_range_and_freshness() {
        let root = workspace();
        let path = root.join("src.rs");
        fs::write(&path, "alpha\nbeta\ngamma\n").expect("write source");
        let proposal = prepare_replace_lines(&root, "src.rs", 2, 2, "beta", "BETA")
            .expect("prepare line edit");
        assert_eq!(proposal.updated, "alpha\nBETA\ngamma\n");
        assert!(prepare_replace_lines(&root, "src.rs", 2, 2, "gamma", "BETA").is_err());
        assert!(prepare_replace_lines(&root, "src.rs", 0, 1, "alpha", "A").is_err());
        let proposal =
            prepare_replace_lines(&root, "src.rs", 2, 2, "beta", "BETA").expect("prepare edit");
        fs::write(&path, "changed concurrently\n").expect("modify source");
        assert!(apply_edit(&root, &proposal).is_err());
        assert_eq!(
            fs::read_to_string(path).expect("read source"),
            "changed concurrently\n"
        );
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn write_inserts_at_a_one_based_line_or_zero_in_an_empty_file() {
        let root = workspace();
        let path = root.join("src.rs");
        fs::write(&path, "alpha\ngamma\n").expect("write source");
        let proposal = prepare_write_to_file(&root, "src.rs", 2, "beta").expect("prepare insert");
        assert_eq!(proposal.updated, "alpha\nbeta\ngamma\n");
        fs::write(&path, &proposal.updated).expect("apply fixture insert");
        assert!(prepare_write_to_file(&root, "src.rs", 0, "bad").is_err());

        fs::write(&path, "").expect("empty source");
        let proposal =
            prepare_write_to_file(&root, "src.rs", 0, "first line").expect("empty insert");
        assert_eq!(proposal.updated, "first line");
        fs::remove_dir_all(root).expect("remove workspace");
    }

    #[test]
    fn create_file_writes_once_and_never_overwrites() {
        let root = workspace();
        let proposal = prepare_create_file(&root, "new.rs", "fn main() {}\n").expect("proposal");
        apply_create(&root, &proposal).expect("create file");
        assert_eq!(
            fs::read_to_string(root.join("new.rs")).expect("read new file"),
            "fn main() {}\n"
        );
        assert!(prepare_create_file(&root, "new.rs", "replacement").is_err());
        assert!(apply_create(&root, &proposal).is_err());
        fs::remove_dir_all(root).expect("remove workspace");
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::run_command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn cancelled_command_is_killed() {
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            trigger.store(true, Ordering::Relaxed);
        });
        let command = if cfg!(windows) {
            "Start-Sleep -Seconds 20"
        } else {
            "sleep 20"
        };
        let started = Instant::now();
        let error = run_command(&std::env::temp_dir(), command, &cancel).expect_err("cancelled");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "took {:?}",
            started.elapsed()
        );
        assert!(error.to_string().contains("cancelled"), "{error}");
    }
}
