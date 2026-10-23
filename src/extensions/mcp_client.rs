//! One connection to an MCP server over its standard input and output: newline-delimited
//! JSON-RPC 2.0. The client starts the server, performs the handshake, lists its tools and
//! calls them. A server cannot ask anything of Cool Code: it declares no client capabilities,
//! answers `ping` and refuses every other request.
//!
//! Everything a server sends is untrusted: lines and total output are capped, waits have
//! deadlines, the process is killed when the client is dropped, and text is stripped of
//! terminal escape sequences before anyone sees it.

use crate::extensions::mcp::ServerInfo;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The protocol versions this client speaks, newest first.
const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Longest line a server may print (a tool result can be large).
const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Most output a server may print in all before it is stopped.
const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
/// Most tools taken from one server.
pub(crate) const MAX_TOOLS: usize = 64;
/// Most pages of the tool list followed.
const MAX_PAGES: usize = 8;
/// Longest tool description kept, in characters.
const MAX_DESCRIPTION_CHARS: usize = 1024;
/// Largest input schema accepted, in bytes.
const MAX_SCHEMA_BYTES: usize = 16 * 1024;
/// Most text of one tool result handed on, in bytes.
pub(crate) const MAX_RESULT_BYTES: usize = 24 * 1024;

/// How long to wait for the server.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    /// For the handshake and each page of the tool list.
    pub(crate) start: Duration,
    /// For one tool call.
    pub(crate) call: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            start: Duration::from_secs(30),
            call: Duration::from_secs(60),
        }
    }
}

/// A tool a server offers.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tool {
    pub(crate) name: String,
    pub(crate) description: String,
    /// The JSON Schema of its arguments (always an object schema).
    pub(crate) schema: Value,
    /// The server's own claim that the tool does not change anything. Only shown to the user:
    /// a server's claim never lets a call skip the approval.
    pub(crate) read_only_hint: bool,
}

/// What a tool call returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolOutput {
    /// The text, with escape sequences and control characters removed and cut to a limit.
    pub(crate) text: String,
    /// The server said the call failed.
    pub(crate) is_error: bool,
}

type Reply = Result<Value, String>;

struct Inner {
    id: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<u64, Sender<Reply>>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    /// Why the connection ended, once it has.
    ended: Mutex<Option<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Inner {
    fn send(&self, message: &Value) -> Result<(), String> {
        let mut slot = lock(&self.stdin);
        let stdin = slot.as_mut().ok_or_else(|| self.reason())?;
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|_| self.reason())
    }

    fn reason(&self) -> String {
        lock(&self.ended)
            .clone()
            .unwrap_or_else(|| "the server is not running".to_owned())
    }

    /// Ends the connection: remembers why, wakes every waiting call and stops the process.
    fn end(&self, reason: &str) {
        {
            let mut ended = lock(&self.ended);
            if ended.is_none() {
                *ended = Some(reason.to_owned());
            }
        }
        self.closed.store(true, Ordering::Relaxed);
        lock(&self.pending).clear();
        *lock(&self.stdin) = None;
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A running, handshaken server.
pub(crate) struct Client {
    inner: Arc<Inner>,
    timeouts: Timeouts,
}

impl Client {
    /// Starts the server and returns it with its tools (and a warning for each tool left out).
    /// `variables` are the only secrets it receives, on top of the harmless ones of the
    /// environment.
    pub(crate) fn connect(
        info: &ServerInfo,
        variables: &[(String, String)],
        timeouts: Timeouts,
    ) -> Result<(Client, Vec<Tool>, Vec<String>), String> {
        let program = crate::extensions::mod_host::program(&info.manifest.command, &info.dir)?;
        let mut child =
            crate::extensions::mod_host::scrubbed_command(program, &info.manifest.args, &info.dir)
                .envs(variables.iter().map(|(name, value)| (name, value)))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("could not start: {error}"))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let inner = Arc::new(Inner {
            id: info.id.clone(),
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            ended: Mutex::new(None),
        });
        if let Some(stdout) = stdout {
            let reader = inner.clone();
            std::thread::spawn(move || read_loop(stdout, &reader));
        }
        let client = Client { inner, timeouts };
        match client.handshake() {
            Ok((tools, warnings)) => Ok((client, tools, warnings)),
            Err(error) => Err(error),
        }
    }

    fn handshake(&self) -> Result<(Vec<Tool>, Vec<String>), String> {
        let started = self.request(
            "initialize",
            json!({
                "protocolVersion": SUPPORTED_VERSIONS[0],
                "capabilities": {},
                "clientInfo": {"name": "coolcode", "version": env!("CARGO_PKG_VERSION")},
            }),
            self.timeouts.start,
            None,
        )?;
        let version = started
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !SUPPORTED_VERSIONS.contains(&version) {
            self.close();
            return Err(format!(
                "speaks MCP version {version:?}, which Cool Code does not support"
            ));
        }
        self.notify("notifications/initialized", json!({}))?;
        let mut tools = Vec::new();
        let mut warnings = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let page = self.request("tools/list", params, self.timeouts.start, None)?;
            let (found, mut skipped) = parse_tools(&page);
            tools.extend(found);
            warnings.append(&mut skipped);
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        if tools.len() > MAX_TOOLS {
            warnings.push(format!(
                "offers more than {MAX_TOOLS} tools; the rest are left out"
            ));
            tools.truncate(MAX_TOOLS);
        }
        Ok((tools, warnings))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.inner
            .send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Sends a request and waits for its answer, giving up at `timeout` or when `cancel` is set.
    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&AtomicBool>,
    ) -> Result<Value, String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_to, answers): (Sender<Reply>, Receiver<Reply>) = std::sync::mpsc::channel();
        lock(&self.inner.pending).insert(id, reply_to);
        let sent = self
            .inner
            .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if let Err(error) = sent {
            lock(&self.inner.pending).remove(&id);
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        let give_up = |why: String| {
            lock(&self.inner.pending).remove(&id);
            let _ = self.notify(
                "notifications/cancelled",
                json!({"requestId": id, "reason": why}),
            );
            Err(why)
        };
        loop {
            match answers.recv_timeout(Duration::from_millis(50)) {
                Ok(reply) => return reply,
                Err(RecvTimeoutError::Disconnected) => return Err(self.inner.reason()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return give_up("cancelled".to_owned());
            }
            if Instant::now() >= deadline {
                return give_up(format!("timed out after {} s", timeout.as_secs().max(1)));
            }
        }
    }

    /// Calls a tool. `arguments` must be a JSON object.
    pub(crate) fn call_tool(
        &self,
        tool: &str,
        arguments: &Value,
        cancel: &AtomicBool,
    ) -> Result<ToolOutput, String> {
        let result = self.request(
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
            self.timeouts.call,
            Some(cancel),
        )?;
        Ok(parse_output(&result))
    }

    /// Whether the connection has ended.
    pub(crate) fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    /// Why the connection ended, if it has.
    pub(crate) fn why_closed(&self) -> Option<String> {
        lock(&self.inner.ended).clone()
    }

    /// Stops the server.
    pub(crate) fn close(&self) {
        self.inner.end("it was stopped");
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads the server's output until it ends: answers go to whoever waits for them, requests from
/// the server are answered or refused, and everything else is ignored.
fn read_loop(stdout: impl Read, inner: &Inner) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut total = 0usize;
    let reason = loop {
        line.clear();
        let Ok(read) = (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
        else {
            break "its output could not be read".to_owned();
        };
        if read == 0 {
            break "it closed its output".to_owned();
        }
        total += read;
        if line.len() > MAX_LINE_BYTES {
            break "it printed a line longer than 1 MiB and was stopped".to_owned();
        }
        if total > MAX_OUTPUT_BYTES {
            break "it printed more than 64 MiB and was stopped".to_owned();
        }
        let Ok(message) = serde_json::from_slice::<Value>(&line) else {
            // Not JSON: servers sometimes print a banner or a log line. Ignore it.
            continue;
        };
        route(inner, &message);
    };
    inner.end(&reason);
}

/// Hands one message from the server to whoever waits for it.
fn route(inner: &Inner, message: &Value) {
    let id = message.get("id");
    let method = message.get("method").and_then(Value::as_str);
    match (id, method) {
        // A request from the server: it may only ping.
        (Some(id), Some(method)) => {
            let answer = if method == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": "Cool Code does not support this request"},
                })
            };
            let _ = inner.send(&answer);
        }
        // An answer to one of ours.
        (Some(id), None) => {
            let Some(id) = id.as_u64() else { return };
            let Some(reply_to) = lock(&inner.pending).remove(&id) else {
                return;
            };
            let reply = match message.get("error") {
                Some(error) => Err(error_text(error)),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = reply_to.send(reply);
        }
        // A notification (progress, logging, list changes): nothing in this release uses them.
        _ => {}
    }
}

fn error_text(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the server reported an error");
    format!(
        "the server reported an error: {}",
        clean_output(message, 500)
    )
}

/// The tools of one page of `tools/list`, and a warning for each that was left out.
fn parse_tools(page: &Value) -> (Vec<Tool>, Vec<String>) {
    let mut tools = Vec::new();
    let mut warnings = Vec::new();
    for entry in page
        .get("tools")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if name.is_empty() || name.len() > 128 {
            warnings.push("a tool without a usable name was left out".to_owned());
            continue;
        }
        let schema = entry
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"}));
        if !schema.is_object() || schema.get("type").and_then(Value::as_str) != Some("object") {
            warnings.push(format!(
                "the tool {name} was left out: its input is not an object"
            ));
            continue;
        }
        if schema.to_string().len() > MAX_SCHEMA_BYTES {
            warnings.push(format!(
                "the tool {name} was left out: its input schema is over 16 KiB"
            ));
            continue;
        }
        let description = entry
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        tools.push(Tool {
            name: name.to_owned(),
            description: clean_output(description, MAX_DESCRIPTION_CHARS * 4)
                .chars()
                .take(MAX_DESCRIPTION_CHARS)
                .collect(),
            schema,
            read_only_hint: entry
                .pointer("/annotations/readOnlyHint")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    (tools, warnings)
}

/// The text of a `tools/call` result. Anything that is not text (images, audio, files) is
/// replaced by a short note.
fn parse_output(result: &Value) -> ToolOutput {
    let mut parts = Vec::new();
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                parts.push(
                    item.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
            Some(kind @ ("image" | "audio")) => parts.push(format!("[{kind} omitted]")),
            Some("resource") => parts.push("[resource omitted]".to_owned()),
            Some("resource_link") => parts.push("[resource link omitted]".to_owned()),
            _ => parts.push("[content omitted]".to_owned()),
        }
    }
    if parts.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        parts.push(structured.to_string());
    }
    ToolOutput {
        text: clean_output(&parts.join("\n"), MAX_RESULT_BYTES),
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// `text` made safe to show and to hand to a model: terminal escape sequences, other control
/// characters (line breaks and tabs stay) and direction overrides removed, and cut to
/// `max_bytes` with a note.
pub(crate) fn clean_output(text: &str, max_bytes: usize) -> String {
    let mut cleaned = String::with_capacity(text.len().min(max_bytes + 16));
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\u{1b}' => skip_escape(&mut characters),
            '\n' | '\t' => cleaned.push(character),
            '\r' => {}
            c if c.is_control() => {}
            '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            c => cleaned.push(c),
        }
    }
    if cleaned.len() <= max_bytes {
        return cleaned;
    }
    let mut end = max_bytes;
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    cleaned.truncate(end);
    cleaned.push_str("\n[output cut]");
    cleaned
}

/// Skips what follows an escape character: a `[...` sequence up to its final letter, an
/// `]...` operating-system command up to its terminator, or one more character.
fn skip_escape(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match characters.next() {
        Some('[') => {
            for next in characters.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&next) {
                    break;
                }
            }
        }
        Some(']') => {
            while let Some(next) = characters.next() {
                if next == '\u{7}' {
                    break;
                }
                if next == '\u{1b}' {
                    characters.next();
                    break;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::extensions::mcp::{ServerInfo, ServerManifest, Source};
    use std::io::BufRead as _;

    /// Marks the test binary's own run as an MCP server (see [`mcp_helper_process`]).
    const HELPER: &str = "coolcode-mcp-helper";

    pub(crate) fn quick() -> Timeouts {
        Timeouts {
            start: Duration::from_secs(10),
            call: Duration::from_secs(10),
        }
    }

    /// This test binary, run as an MCP server in one of several behaviours.
    pub(crate) fn helper(mode: &str) -> ServerInfo {
        let exe = std::env::current_exe().expect("test binary");
        ServerInfo::new(
            Source::User,
            ServerManifest {
                name: format!("helper-{mode}"),
                command: exe.display().to_string(),
                args: [
                    "--exact",
                    "extensions::mcp_client::tests::mcp_helper_process",
                    "--nocapture",
                    "--test-threads=1",
                    HELPER,
                    mode,
                ]
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect(),
                ..Default::default()
            },
            std::env::temp_dir(),
        )
    }

    fn write(out: &mut impl Write, value: Value) {
        writeln!(out, "{value}").unwrap();
        out.flush().unwrap();
    }

    /// Not a test of its own: when the client starts this binary as a server, this is the server.
    #[test]
    fn mcp_helper_process() {
        let args = std::env::args().collect::<Vec<_>>();
        if !args.iter().any(|arg| arg == HELPER) {
            return;
        }
        let mode = args.last().cloned().unwrap_or_default();
        let mut out = std::io::stdout();
        // The test runner has printed "test <name> ... " without a line break; end that line.
        writeln!(out).unwrap();
        let tool = |name: &str| {
            json!({
                "name": name,
                "description": format!("the {name} tool \u{1b}[31mred"),
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
                "annotations": {"readOnlyHint": name == "echo"},
            })
        };
        let stdin = std::io::stdin();
        let (mut ping, mut refused, mut waiting) = (false, false, None::<Value>);
        for line in stdin.lock().lines() {
            let Ok(message) = serde_json::from_str::<Value>(&line.unwrap()) else {
                continue;
            };
            let id = message.get("id").cloned();
            match id.as_ref().and_then(Value::as_str) {
                Some("srv-1") => ping = message.get("result").is_some(),
                Some("srv-2") => refused = message.pointer("/error/code") == Some(&json!(-32601)),
                _ => {}
            }
            if ping
                && refused
                && let Some(call) = waiting.take()
            {
                write(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": call, "result": {"content": [{"type": "text", "text": format!("ping={ping} refused={refused}")}]}}),
                );
            }
            let method = message.get("method").and_then(Value::as_str).unwrap_or("");
            let reply = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
            match method {
                "initialize" => {
                    if mode == "silent" {
                        continue;
                    }
                    let version = if mode == "old" {
                        "1999-01-01"
                    } else {
                        "2025-06-18"
                    };
                    write(
                        &mut out,
                        reply(json!({
                            "protocolVersion": version,
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "helper", "version": "1"},
                        })),
                    );
                }
                "tools/list" => {
                    let cursor = message.pointer("/params/cursor").and_then(Value::as_str);
                    let page = match (mode.as_str(), cursor) {
                        ("paged", None) => json!({"tools": [tool("echo")], "nextCursor": "two"}),
                        ("paged", Some(_)) => json!({"tools": [tool("second")]}),
                        ("crash", _) => std::process::exit(3),
                        ("badtools", _) => json!({"tools": [
                            tool("good"),
                            {"name": "", "inputSchema": {"type": "object"}},
                            {"name": "scalar", "inputSchema": {"type": "string"}},
                            {"name": "huge", "inputSchema": {"type": "object", "x": "y".repeat(20_000)}},
                        ]}),
                        ("many", _) => {
                            json!({"tools": (0..80).map(|n| tool(&format!("t{n}"))).collect::<Vec<_>>()})
                        }
                        _ => {
                            json!({"tools": [tool("echo"), tool("boom"), tool("slow"), tool("ansi"), tool("big"), tool("env"), tool("pinger")]})
                        }
                    };
                    write(&mut out, reply(page));
                }
                "tools/call" => {
                    let name = message
                        .pointer("/params/name")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let arguments = message
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or_default();
                    match name {
                        "echo" => write(
                            &mut out,
                            reply(
                                json!({"content": [{"type": "text", "text": format!("echo {arguments}")}]}),
                            ),
                        ),
                        "boom" => write(
                            &mut out,
                            reply(
                                json!({"isError": true, "content": [{"type": "text", "text": "it broke"}]}),
                            ),
                        ),
                        "slow" => {
                            std::thread::sleep(Duration::from_secs(30));
                        }
                        "ansi" => write(
                            &mut out,
                            reply(json!({"content": [
                                {"type": "text", "text": "\u{1b}[2J\u{1b}[31mred\u{1b}[0m \u{1b}]0;title\u{7}ok\u{8}\u{202e}x"},
                                {"type": "image", "data": "AAAA", "mimeType": "image/png"}
                            ]})),
                        ),
                        "big" => write(
                            &mut out,
                            reply(
                                json!({"content": [{"type": "text", "text": "x".repeat(100_000)}]}),
                            ),
                        ),
                        "env" => {
                            let leaked = std::env::var("CARGO_MANIFEST_DIR").is_ok();
                            let token = std::env::var("TOKEN").unwrap_or_default();
                            write(
                                &mut out,
                                reply(
                                    json!({"content": [{"type": "text", "text": format!("leaked={leaked} token={token}")}]}),
                                ),
                            );
                        }
                        "pinger" => {
                            // Ask the client for a ping; the call is answered once it replied.
                            write(
                                &mut out,
                                json!({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"}),
                            );
                            write(
                                &mut out,
                                json!({"jsonrpc": "2.0", "id": "srv-2", "method": "sampling/createMessage", "params": {}}),
                            );
                            waiting = id.clone();
                        }
                        _ => write(
                            &mut out,
                            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "unknown tool"}}),
                        ),
                    }
                }
                _ => {}
            }
            if mode == "flood" && method == "initialize" {
                write(&mut out, json!({"junk": "z".repeat(MAX_LINE_BYTES + 10)}));
            }
        }
    }

    fn connect(mode: &str) -> (Client, Vec<Tool>, Vec<String>) {
        Client::connect(&helper(mode), &[], quick()).expect("connects")
    }

    #[test]
    fn it_handshakes_lists_tools_and_calls_one() {
        let (client, tools, warnings) = connect("basic");
        assert!(warnings.is_empty(), "{warnings:?}");
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names[..2], ["echo", "boom"]);
        assert!(tools[0].read_only_hint && !tools[1].read_only_hint);
        assert_eq!(tools[0].schema["type"], "object");
        assert!(
            !tools[0].description.contains('\u{1b}'),
            "descriptions are cleaned: {:?}",
            tools[0].description
        );
        let cancel = AtomicBool::new(false);
        let output = client
            .call_tool("echo", &json!({"text": "hi"}), &cancel)
            .expect("call");
        assert_eq!(output.text, r#"echo {"text":"hi"}"#);
        assert!(!output.is_error);
        let failed = client.call_tool("boom", &json!({}), &cancel).expect("call");
        assert!(failed.is_error);
        assert_eq!(failed.text, "it broke");
        let unknown = client.call_tool("nothing", &json!({}), &cancel);
        assert!(unknown.unwrap_err().contains("unknown tool"));
    }

    #[test]
    fn a_paged_tool_list_is_followed() {
        let (_, tools, _) = connect("paged");
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["echo", "second"]);
    }

    #[test]
    fn unusable_tools_are_left_out_with_a_warning_each() {
        let (_, tools, warnings) = connect("badtools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "good");
        assert_eq!(warnings.len(), 3, "{warnings:?}");
    }

    #[test]
    fn at_most_sixty_four_tools_are_taken() {
        let (_, tools, warnings) = connect("many");
        assert_eq!(tools.len(), MAX_TOOLS);
        assert!(
            warnings.iter().any(|w| w.contains("more than 64")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_version_cool_code_does_not_speak_is_refused() {
        let error = Client::connect(&helper("old"), &[], quick())
            .err()
            .expect("refused");
        assert!(error.contains("does not support"), "{error}");
    }

    #[test]
    fn a_server_that_never_answers_is_given_up_on() {
        let timeouts = Timeouts {
            start: Duration::from_millis(300),
            call: Duration::from_secs(1),
        };
        let started = Instant::now();
        let error = Client::connect(&helper("silent"), &[], timeouts)
            .err()
            .expect("gives up");
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_server_that_exits_during_the_handshake_is_reported() {
        let error = Client::connect(&helper("crash"), &[], quick())
            .err()
            .expect("fails");
        assert!(error.contains("closed its output"), "{error}");
    }

    #[test]
    fn a_slow_call_times_out_and_a_cancel_stops_the_wait() {
        let timeouts = Timeouts {
            start: Duration::from_secs(10),
            call: Duration::from_millis(400),
        };
        let (client, _, _) = Client::connect(&helper("basic"), &[], timeouts).expect("connects");
        let cancel = AtomicBool::new(false);
        let error = client.call_tool("slow", &json!({}), &cancel).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        let (client, _, _) = connect("basic");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let error = client.call_tool("slow", &json!({}), &stop).unwrap_err();
        assert_eq!(error, "cancelled");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_line_over_the_limit_stops_the_server() {
        // The handshake answer is followed by a huge line: the connection ends right there.
        let error = Client::connect(&helper("flood"), &[], quick())
            .err()
            .expect("stopped");
        assert!(error.contains("longer than 1 MiB"), "{error}");
    }

    #[test]
    fn the_server_gets_only_the_variables_it_was_given() {
        let (client, _, _) = Client::connect(
            &helper("basic"),
            &[("TOKEN".to_owned(), "s3cret".to_owned())],
            quick(),
        )
        .expect("connects");
        let cancel = AtomicBool::new(false);
        let output = client.call_tool("env", &json!({}), &cancel).unwrap();
        assert_eq!(output.text, "leaked=false token=s3cret");
    }

    #[test]
    fn it_answers_a_ping_and_refuses_every_other_request_from_the_server() {
        let (client, _, _) = connect("basic");
        let cancel = AtomicBool::new(false);
        let output = client.call_tool("pinger", &json!({}), &cancel).unwrap();
        assert_eq!(output.text, "ping=true refused=true");
    }

    #[test]
    fn results_are_cleaned_and_cut() {
        let (client, _, _) = connect("basic");
        let cancel = AtomicBool::new(false);
        let output = client.call_tool("ansi", &json!({}), &cancel).unwrap();
        assert_eq!(output.text, "red okx\n[image omitted]");
        let big = client.call_tool("big", &json!({}), &cancel).unwrap();
        assert!(big.text.len() < MAX_RESULT_BYTES + 32, "{}", big.text.len());
        assert!(big.text.ends_with("[output cut]"));
        assert_eq!(
            clean_output("héllo wörld", 2),
            "h\n[output cut]",
            "cut on a character boundary"
        );
    }

    #[test]
    fn the_server_is_stopped_when_the_client_is_dropped() {
        let (client, _, _) = connect("basic");
        let pid = lock(&client.inner.child).as_ref().expect("child").id();
        drop(client);
        // A killed and reaped process no longer exists: signal 0 only checks that.
        #[cfg(unix)]
        {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            assert!(!alive, "process {pid} is still running");
        }
        #[cfg(not(unix))]
        let _ = pid;
    }
}
