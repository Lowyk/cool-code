//! Saved conversations that can be resumed later.
//!
//! Each session is one file under `~/.coolcode/sessions/`. The first line is a small header (so a
//! list of sessions never parses whole conversations); the second line holds the messages and the
//! transcript. Files are written to a temporary name and renamed, so a crash cannot leave half a
//! session behind.

use crate::provider::ChatMessage;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Refuse to read session files larger than this.
const MAX_SESSION_BYTES: u64 = 256 * 1024 * 1024;
/// A session list never reads more than this many files.
const MAX_LISTED: usize = 500;
const TITLE_CHARS: usize = 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Header {
    pub(crate) id: String,
    pub(crate) cwd: String,
    pub(crate) created: i64,
    pub(crate) updated: i64,
    pub(crate) title: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    /// Number of messages the user typed.
    pub(crate) prompts: usize,
}

/// `ChatMessage` with every field kept (the live type skips some when talking to providers).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredMessage {
    role: String,
    content: Value,
    tool_calls: Option<Vec<Value>>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    display: String,
    thought_signatures: Vec<(String, String)>,
}

impl From<&ChatMessage> for StoredMessage {
    fn from(message: &ChatMessage) -> Self {
        StoredMessage {
            role: message.role.clone(),
            content: message.content.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            tool_name: message.tool_name.clone(),
            display: message.display.clone(),
            thought_signatures: message.thought_signatures.clone(),
        }
    }
}

impl From<StoredMessage> for ChatMessage {
    fn from(stored: StoredMessage) -> Self {
        ChatMessage {
            role: stored.role,
            content: stored.content,
            tool_calls: stored.tool_calls,
            tool_call_id: stored.tool_call_id,
            tool_name: stored.tool_name,
            display: stored.display,
            thought_signatures: stored.thought_signatures,
        }
    }
}

/// One line of the on-screen transcript. `kind` is one of `user`, `assistant`, `command`, `error`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredEntry {
    pub(crate) kind: String,
    pub(crate) text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Body {
    messages: Vec<StoredMessage>,
    transcript: Vec<StoredEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Session {
    pub(crate) header: Header,
    pub(crate) messages: Vec<StoredMessage>,
    pub(crate) transcript: Vec<StoredEntry>,
}

#[cfg(not(test))]
pub(crate) fn default_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not locate the home directory")?;
    Ok(home.join(".coolcode").join("sessions"))
}

// Every call gets a fresh directory so tests never touch real sessions or each other.
#[cfg(test)]
pub(crate) fn default_dir() -> Result<PathBuf> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    // Process ids are reused, so a folder left by an earlier run must never be picked up again.
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "harness-test-{}-{started}-sessions-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Ok(dir)
}

/// A new session id: sortable by time, unique enough for one person's machine.
pub(crate) fn new_id(now: i64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    format!("s{now}-{:04x}", (nanos ^ std::process::id()) & 0xffff)
}

/// Ids become file names, so only plain characters are accepted.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn path_for(dir: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        bail!("`{id}` is not a valid session id");
    }
    Ok(dir.join(format!("{id}.json")))
}

/// The first thing the user said, flattened to one short line.
pub(crate) fn title_from(transcript: &[StoredEntry]) -> String {
    let first = transcript
        .iter()
        .find(|entry| entry.kind == "user")
        .map_or("", |entry| entry.text.as_str());
    let flat = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= TITLE_CHARS {
        flat
    } else {
        format!(
            "{}…",
            flat.chars().take(TITLE_CHARS - 1).collect::<String>()
        )
    }
}

pub(crate) fn save_in(dir: &Path, session: &Session) -> Result<()> {
    let path = path_for(dir, &session.header.id)?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let body = Body {
        messages: session.messages.clone(),
        transcript: session.transcript.clone(),
    };
    let text = format!(
        "{}\n{}\n",
        serde_json::to_string(&session.header).context("serializing the session header")?,
        serde_json::to_string(&body).context("serializing the session")?
    );
    let temporary = dir.join(format!(".{}.tmp", session.header.id));
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, &path).with_context(|| format!("saving {}", path.display()))
}

fn read_header(path: &Path) -> Option<Header> {
    let mut line = String::new();
    BufReader::new(std::fs::File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str(&line).ok()
}

/// Sessions, newest first; `cwd` limits them to one folder. Unreadable files are skipped.
pub(crate) fn list_in(dir: &Path, cwd: Option<&str>) -> Vec<Header> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut headers = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .take(MAX_LISTED)
        .filter_map(|path| read_header(&path))
        .filter(|header| valid_id(&header.id) && cwd.is_none_or(|cwd| header.cwd == cwd))
        .collect::<Vec<_>>();
    headers.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    headers
}

pub(crate) fn load_in(dir: &Path, id: &str) -> Result<Session> {
    let path = path_for(dir, id)?;
    let size = std::fs::metadata(&path)
        .with_context(|| format!("opening {}", path.display()))?
        .len();
    if size > MAX_SESSION_BYTES {
        bail!("session {id} is too large to load ({size} bytes)");
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut lines = text.lines();
    let header: Header = serde_json::from_str(lines.next().unwrap_or(""))
        .with_context(|| format!("{} has no readable header", path.display()))?;
    let body: Body = serde_json::from_str(lines.next().unwrap_or(""))
        .with_context(|| format!("{} has no readable conversation", path.display()))?;
    if header.id != id {
        bail!("session file {id} belongs to a different session");
    }
    Ok(Session {
        header,
        messages: body.messages,
        transcript: body.transcript,
    })
}

pub(crate) fn delete_in(dir: &Path, id: &str) -> Result<()> {
    let path = path_for(dir, id)?;
    std::fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))
}

/// "5 minutes ago", for the session list.
pub(crate) fn ago(now: i64, then: i64) -> String {
    let seconds = now.saturating_sub(then).max(0);
    let (value, unit) = match seconds {
        0..=59 => return "just now".to_owned(),
        60..=3_599 => (seconds / 60, "minute"),
        3_600..=86_399 => (seconds / 3_600, "hour"),
        86_400..=2_591_999 => (seconds / 86_400, "day"),
        _ => (seconds / 2_592_000, "month"),
    };
    format!("{value} {unit}{} ago", if value == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "harness-session-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn session(id: &str, cwd: &str, updated: i64, first_prompt: &str) -> Session {
        Session {
            header: Header {
                id: id.to_owned(),
                cwd: cwd.to_owned(),
                created: updated - 10,
                updated,
                title: String::new(),
                provider: "MultiAI".to_owned(),
                model: "claude-opus-5-5".to_owned(),
                prompts: 1,
            },
            messages: vec![StoredMessage {
                role: "user".to_owned(),
                content: json!(first_prompt),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
                display: first_prompt.to_owned(),
                thought_signatures: Vec::new(),
            }],
            transcript: vec![StoredEntry {
                kind: "user".to_owned(),
                text: first_prompt.to_owned(),
            }],
        }
    }

    #[test]
    fn a_saved_session_loads_back_exactly() {
        let dir = dir("roundtrip");
        let mut original = session("s100-abcd", "/work", 100, "hello there");
        original.messages.push(StoredMessage {
            role: "tool".to_owned(),
            content: json!("file contents"),
            tool_calls: Some(vec![json!({"id": "call_1"})]),
            tool_call_id: Some("call_1".to_owned()),
            tool_name: Some("read_file".to_owned()),
            display: "Read a.txt".to_owned(),
            thought_signatures: vec![("call_1".to_owned(), "sig".to_owned())],
        });
        save_in(&dir, &original).expect("save");
        assert_eq!(load_in(&dir, "s100-abcd").expect("load"), original);
        // Nothing but the session file is left behind.
        let names = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["s100-abcd.json"]);
    }

    #[test]
    fn live_messages_survive_a_round_trip_with_their_skipped_fields() {
        let mut message = ChatMessage::assistant("hi".to_owned());
        message.tool_name = Some("grep".to_owned());
        message.display = "Searched".to_owned();
        message.thought_signatures = vec![("c".to_owned(), "s".to_owned())];
        let back = ChatMessage::from(StoredMessage::from(&message));
        assert_eq!(back.tool_name.as_deref(), Some("grep"));
        assert_eq!(back.display, "Searched");
        assert_eq!(back.thought_signatures, message.thought_signatures);
        assert_eq!(back.role, "assistant");
    }

    #[test]
    fn listing_is_newest_first_scoped_by_folder_and_skips_bad_files() {
        let dir = dir("listing");
        save_in(&dir, &session("s1-aaaa", "/a", 100, "old")).unwrap();
        save_in(&dir, &session("s2-bbbb", "/a", 300, "new")).unwrap();
        save_in(&dir, &session("s3-cccc", "/b", 200, "other folder")).unwrap();
        std::fs::write(dir.join("junk.json"), "not json").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignore me").unwrap();
        let ids = |cwd| {
            list_in(&dir, cwd)
                .into_iter()
                .map(|header| header.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(Some("/a")), ["s2-bbbb", "s1-aaaa"]);
        assert_eq!(ids(None), ["s2-bbbb", "s3-cccc", "s1-aaaa"]);
        assert!(ids(Some("/nowhere")).is_empty());
        assert!(list_in(&dir.join("missing"), None).is_empty());
    }

    #[test]
    fn deleting_removes_the_session() {
        let dir = dir("delete");
        save_in(&dir, &session("s1-aaaa", "/a", 1, "x")).unwrap();
        delete_in(&dir, "s1-aaaa").unwrap();
        assert!(list_in(&dir, None).is_empty());
        assert!(delete_in(&dir, "s1-aaaa").is_err());
    }

    #[test]
    fn ids_cannot_escape_the_sessions_folder() {
        let dir = dir("ids");
        for bad in ["../x", "a/b", "a\\b", "", "has space", "x.json"] {
            assert!(load_in(&dir, bad).is_err(), "{bad:?}");
            assert!(delete_in(&dir, bad).is_err(), "{bad:?}");
            assert!(
                save_in(&dir, &session(bad, "/a", 1, "x")).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_file_whose_header_names_another_session_is_refused() {
        let dir = dir("mismatch");
        save_in(&dir, &session("s1-aaaa", "/a", 1, "x")).unwrap();
        std::fs::rename(dir.join("s1-aaaa.json"), dir.join("s2-bbbb.json")).unwrap();
        assert!(load_in(&dir, "s2-bbbb").is_err());
    }

    #[test]
    fn corrupt_session_files_fail_to_load_with_a_message() {
        let dir = dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s1-aaaa.json"), "{}\nnot json\n").unwrap();
        assert!(load_in(&dir, "s1-aaaa").is_err());
        std::fs::write(dir.join("s2-bbbb.json"), "").unwrap();
        assert!(load_in(&dir, "s2-bbbb").is_err());
    }

    #[test]
    fn saving_again_replaces_the_session() {
        let dir = dir("overwrite");
        let mut first = session("s1-aaaa", "/a", 1, "x");
        save_in(&dir, &first).unwrap();
        first.header.updated = 50;
        first.header.title = "renamed".to_owned();
        save_in(&dir, &first).unwrap();
        let loaded = load_in(&dir, "s1-aaaa").unwrap();
        assert_eq!(loaded.header.updated, 50);
        assert_eq!(list_in(&dir, None).len(), 1);
    }

    #[test]
    fn titles_are_the_first_prompt_flattened_and_shortened() {
        let entries = |text: &str| {
            vec![
                StoredEntry {
                    kind: "command".to_owned(),
                    text: "ignored".to_owned(),
                },
                StoredEntry {
                    kind: "user".to_owned(),
                    text: text.to_owned(),
                },
            ]
        };
        assert_eq!(title_from(&entries("fix\n  the   bug")), "fix the bug");
        let long = "word ".repeat(40);
        let title = title_from(&entries(&long));
        assert_eq!(title.chars().count(), TITLE_CHARS);
        assert!(title.ends_with('…'));
        assert_eq!(title_from(&[]), "");
    }

    #[test]
    fn new_ids_are_valid_and_sort_by_time() {
        let early = new_id(1_000);
        let late = new_id(2_000);
        assert!(valid_id(&early) && valid_id(&late));
        assert!(early < late);
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(1_000, 990), "just now");
        assert_eq!(ago(1_000, 940), "1 minute ago");
        assert_eq!(ago(10_000, 0), "2 hours ago");
        assert_eq!(ago(200_000, 0), "2 days ago");
        assert_eq!(ago(100_000_000, 0), "38 months ago");
        assert_eq!(ago(0, 500), "just now");
    }
}
