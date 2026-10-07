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

#[derive(Clone, Debug)]
pub(crate) struct ToolDefinition {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) parameters: Value,
}

pub(crate) fn definitions() -> [ToolDefinition; 9] {
    [
        ToolDefinition {
            name: "list_files",
            description: "List workspace source files, excluding hidden and generated directories.",
            parameters: serde_json::json!({"type":"object", "properties":{}, "additionalProperties":false}),
        },
        ToolDefinition {
            name: "read_file",
            description: "Read a UTF-8 file inside the trusted workspace. Paths must be workspace-relative.",
            parameters: serde_json::json!({"type":"object", "properties":{"path":{"type":"string"}}, "required":["path"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "search_text",
            description: "Search text files in the trusted workspace for a literal case-insensitive phrase.",
            parameters: serde_json::json!({"type":"object", "properties":{"query":{"type":"string"}}, "required":["query"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "git_status",
            description: "Read-only Git status for the current workspace.",
            parameters: serde_json::json!({"type":"object", "properties":{}, "additionalProperties":false}),
        },
        ToolDefinition {
            name: "replace_in_file",
            description: "Replace text on a specific inclusive one-based line range. Supply the exact expected_text read from those lines so the harness can detect stale context, plus the replacement.",
            parameters: serde_json::json!({"type":"object", "properties":{"path":{"type":"string"}, "start_line":{"type":"integer", "minimum":1}, "end_line":{"type":"integer", "minimum":1}, "expected_text":{"type":"string"}, "replacement":{"type":"string"}}, "required":["path","start_line","end_line","expected_text","replacement"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "write_to_file",
            description: "Insert text/code at a specific line in an existing file. Lines are one-based; line 0 is allowed only for an empty file.",
            parameters: serde_json::json!({"type":"object", "properties":{"path":{"type":"string"}, "line_number":{"type":"integer", "minimum":0}, "text":{"type":"string"}}, "required":["path","line_number","text"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "create_file",
            description: "Create a new file with complete initial contents. Fails rather than overwriting an existing file.",
            parameters: serde_json::json!({"type":"object", "properties":{"path":{"type":"string"}, "content":{"type":"string"}}, "required":["path","content"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "run_command",
            description: "Run a command in the trusted workspace through the platform's configured shell. The harness will apply permission rules and may ask the user first.",
            parameters: serde_json::json!({"type":"object", "properties":{"command":{"type":"string"}}, "required":["command"], "additionalProperties":false}),
        },
        ToolDefinition {
            name: "request_plan_approval",
            description: "In Plan mode, present an exact ordered list of line edits, file writes/creates, and commands for user approval before performing any of them.",
            parameters: serde_json::json!({"type":"object", "properties":{"summary":{"type":"string"}, "actions":{"type":"array", "items":{"type":"object", "properties":{"name":{"type":"string", "enum":["replace_in_file", "write_to_file", "create_file", "run_command"]}, "arguments":{"type":"object"}}, "required":["name", "arguments"], "additionalProperties":false}}}, "required":["summary", "actions"], "additionalProperties":false}),
        },
    ]
}

pub(crate) fn definitions_for_mode(permission_mode: &str) -> Vec<ToolDefinition> {
    definitions()
        .into_iter()
        .filter(|tool| permission_mode == "plan" || tool.name != "request_plan_approval")
        .collect()
}

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
const MAX_SEARCH_MATCHES: usize = 100;
const MAX_OUTPUT_BYTES: usize = 48 * 1024;

pub(crate) fn list_files(root: &Path) -> Result<String> {
    let root = canonical_root(root)?;
    let mut files = Vec::new();
    let mut visited = 0;
    collect_files(&root, &root, &mut files, &mut visited, 0)?;
    files.sort();
    let limited = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    let mut result = files.join("\n");
    if limited {
        result.push_str(&format!("\n… truncated at {MAX_FILES} files"));
    }
    Ok(if result.is_empty() {
        "No visible files found.".to_owned()
    } else {
        result
    })
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
    if query.trim().is_empty() {
        bail!("search text cannot be empty");
    }
    let root = canonical_root(root)?;
    let mut files = Vec::new();
    let mut visited = 0;
    collect_files(&root, &root, &mut files, &mut visited, 0)?;
    files.sort();
    let mut output = String::new();
    let mut matches = 0usize;
    for relative in files {
        if matches >= MAX_SEARCH_MATCHES || output.len() >= MAX_OUTPUT_BYTES {
            break;
        }
        let path = root.join(&relative);
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_READ_BYTES {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        for (line_index, line) in contents.lines().enumerate() {
            if line.to_lowercase().contains(&query.to_lowercase()) {
                output.push_str(&format!("{relative}:{}: {}\n", line_index + 1, line.trim()));
                matches += 1;
                if matches >= MAX_SEARCH_MATCHES || output.len() >= MAX_OUTPUT_BYTES {
                    break;
                }
            }
        }
    }
    if output.is_empty() {
        Ok("No matches found.".to_owned())
    } else {
        if matches >= MAX_SEARCH_MATCHES || output.len() >= MAX_OUTPUT_BYTES {
            output.push_str("… results truncated\n");
        }
        Ok(output)
    }
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
            validate_arguments(arguments, &[])?;
            list_files(root)?
        }
        "read_file" => {
            validate_arguments(arguments, &["path"])?;
            read_file(root, required_string(arguments, "path")?)?
        }
        "search_text" => {
            validate_arguments(arguments, &["query"])?;
            search(root, required_string(arguments, "query")?)?
        }
        "git_status" => {
            validate_arguments(arguments, &[])?;
            git_status(root)?
        }
        _ => bail!("unknown or unauthorized tool `{name}`"),
    };
    Ok(output.chars().take(MAX_OUTPUT_BYTES).collect())
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
