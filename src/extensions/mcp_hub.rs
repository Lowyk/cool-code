//! The MCP servers that are running: which ones, what state they are in and which tools they
//! offer. The interface starts and stops servers; the agent, on its own thread, asks for the
//! tool list and calls tools through the same shared hub.
//!
//! A server's tools reach the model as `mcp__<server>__<tool>`: names are cut to what model APIs
//! accept (letters, digits, `_` and `-`, at most 64 characters) and made unique.

use crate::extensions::mcp::ServerInfo;
use crate::extensions::mcp_client::{Client, Timeouts, Tool, ToolOutput};
use crate::tools::ToolDefinition;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Prefix of every tool name that comes from a server.
pub(crate) const PREFIX: &str = "mcp__";
/// Longest tool name model APIs accept.
const MAX_NAME: usize = 64;

/// Whether `name` is a tool of an MCP server.
pub(crate) fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(PREFIX)
}

/// How far a server has got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Starting,
    Ready,
    Failed(String),
}

/// What the interface shows about one server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) id: String,
    pub(crate) phase: Phase,
    pub(crate) tools: usize,
    /// Tools or other things the server offered that were left out, and why.
    pub(crate) warnings: Vec<String>,
}

enum State {
    Starting,
    Ready {
        client: Arc<Client>,
        tools: Vec<Tool>,
        warnings: Vec<String>,
    },
    Failed(String),
}

struct Entry {
    info: ServerInfo,
    /// Tells a start that was replaced or stopped meanwhile to give up.
    generation: u64,
    state: State,
}

#[derive(Default)]
struct Servers {
    entries: Vec<Entry>,
    next_generation: u64,
}

/// A tool as the model sees it, with where it really lives.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Listed {
    pub(crate) qualified: String,
    pub(crate) server: String,
    pub(crate) tool: Tool,
}

#[derive(Default)]
pub(crate) struct Hub {
    servers: Arc<Mutex<Servers>>,
}

fn lock(servers: &Mutex<Servers>) -> MutexGuard<'_, Servers> {
    servers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The hub the whole program shares.
pub(crate) fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(Hub::default)
}

impl Hub {
    /// Starts a server without waiting for it. A server with the same id is stopped first.
    pub(crate) fn start(&self, info: ServerInfo, variables: Vec<(String, String)>) {
        self.start_with(info, variables, Timeouts::default());
    }

    pub(crate) fn start_with(
        &self,
        info: ServerInfo,
        variables: Vec<(String, String)>,
        timeouts: Timeouts,
    ) {
        let generation = {
            let mut servers = lock(&self.servers);
            servers.entries.retain(|entry| entry.info.id != info.id);
            servers.next_generation += 1;
            let generation = servers.next_generation;
            servers.entries.push(Entry {
                info: info.clone(),
                generation,
                state: State::Starting,
            });
            generation
        };
        let servers = self.servers.clone();
        std::thread::spawn(move || {
            let connected = Client::connect(&info, &variables, timeouts);
            let mut servers = lock(&servers);
            let Some(entry) = servers
                .entries
                .iter_mut()
                .find(|entry| entry.info.id == info.id && entry.generation == generation)
            else {
                // Stopped or replaced while starting: the client is dropped, which stops it.
                return;
            };
            entry.state = match connected {
                Ok((client, tools, warnings)) => State::Ready {
                    client: Arc::new(client),
                    tools,
                    warnings,
                },
                Err(reason) => State::Failed(reason),
            };
        });
    }

    /// Stops a server and forgets it.
    pub(crate) fn stop(&self, id: &str) {
        let removed = {
            let mut servers = lock(&self.servers);
            let at = servers.entries.iter().position(|entry| entry.info.id == id);
            at.map(|at| servers.entries.remove(at))
        };
        // Dropping the entry (outside the lock) stops the process.
        drop(removed);
    }

    /// Stops every server whose id starts with `prefix` (a plugin's, for example `kit/`).
    pub(crate) fn stop_prefix(&self, prefix: &str) {
        let removed = {
            let mut servers = lock(&self.servers);
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut servers.entries)
                .into_iter()
                .partition(|entry| entry.info.id.starts_with(prefix));
            servers.entries = kept;
            gone
        };
        drop(removed);
    }

    pub(crate) fn stop_all(&self) {
        let removed = std::mem::take(&mut lock(&self.servers).entries);
        drop(removed);
    }

    /// Every server the hub knows, in the order they were started.
    pub(crate) fn statuses(&self) -> Vec<Status> {
        lock(&self.servers)
            .entries
            .iter()
            .map(|entry| match &entry.state {
                State::Starting => Status {
                    id: entry.info.id.clone(),
                    phase: Phase::Starting,
                    tools: 0,
                    warnings: Vec::new(),
                },
                State::Ready {
                    client,
                    tools,
                    warnings,
                } => Status {
                    id: entry.info.id.clone(),
                    phase: match client.why_closed() {
                        Some(reason) => Phase::Failed(reason),
                        None => Phase::Ready,
                    },
                    tools: tools.len(),
                    warnings: warnings.clone(),
                },
                State::Failed(reason) => Status {
                    id: entry.info.id.clone(),
                    phase: Phase::Failed(reason.clone()),
                    tools: 0,
                    warnings: Vec::new(),
                },
            })
            .collect()
    }

    /// How many servers are up.
    pub(crate) fn ready(&self) -> usize {
        self.statuses()
            .iter()
            .filter(|status| status.phase == Phase::Ready)
            .count()
    }

    /// The tools of every server that is up, with their model-facing names.
    pub(crate) fn listed(&self) -> Vec<Listed> {
        let servers = lock(&self.servers);
        let mut used = HashSet::new();
        let mut listed = Vec::new();
        for entry in &servers.entries {
            let State::Ready { client, tools, .. } = &entry.state else {
                continue;
            };
            if client.is_closed() {
                continue;
            }
            for tool in tools {
                listed.push(Listed {
                    qualified: qualify(&entry.info.id, &tool.name, &mut used),
                    server: entry.info.id.clone(),
                    tool: tool.clone(),
                });
            }
        }
        listed
    }

    /// Whether any server offers a tool right now.
    pub(crate) fn any_tools(&self) -> bool {
        !self.listed().is_empty()
    }

    /// The tool definitions to offer the model.
    pub(crate) fn definitions(&self) -> Vec<ToolDefinition> {
        self.listed()
            .into_iter()
            .map(|listed| ToolDefinition {
                name: intern(&listed.qualified),
                description: intern(&format!(
                    "[MCP server {}] {}",
                    listed.server,
                    if listed.tool.description.is_empty() {
                        "(no description)"
                    } else {
                        &listed.tool.description
                    }
                )),
                parameters: listed.tool.schema,
            })
            .collect()
    }

    /// Where a model-facing tool name leads.
    pub(crate) fn find(&self, qualified: &str) -> Option<Listed> {
        self.listed()
            .into_iter()
            .find(|listed| listed.qualified == qualified)
    }

    /// Calls a tool by its model-facing name.
    pub(crate) fn call(
        &self,
        qualified: &str,
        arguments: &Value,
        cancel: &AtomicBool,
    ) -> Result<ToolOutput, String> {
        let listed = self
            .find(qualified)
            .ok_or_else(|| format!("{qualified} is not available: its server is not running"))?;
        let client = {
            let servers = lock(&self.servers);
            servers
                .entries
                .iter()
                .find(|entry| entry.info.id == listed.server)
                .and_then(|entry| match &entry.state {
                    State::Ready { client, .. } => Some(client.clone()),
                    _ => None,
                })
        }
        .ok_or_else(|| format!("{qualified} is not available: its server is not running"))?;
        client.call_tool(&listed.tool.name, arguments, cancel)
    }
}

/// `mcp__<server>__<tool>` made of safe characters, at most 64 long and not in `used`.
fn qualify(server: &str, tool: &str, used: &mut HashSet<String>) -> String {
    let safe = |text: &str| {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let head = format!("{PREFIX}{}__", safe(server));
    let tool_part = safe(tool);
    let mut name = format!("{head}{tool_part}");
    if name.len() > MAX_NAME {
        let room = MAX_NAME.saturating_sub(head.len() + 5);
        name = format!(
            "{head}{}_{}",
            &tool_part[..room.min(tool_part.len())],
            short_hash(server, tool)
        );
        name.truncate(MAX_NAME);
    }
    let mut candidate = name.clone();
    let mut counter = 2;
    while !used.insert(candidate.clone()) {
        let suffix = format!("_{counter}");
        let keep = MAX_NAME.saturating_sub(suffix.len()).min(name.len());
        candidate = format!("{}{suffix}", &name[..keep]);
        counter += 1;
    }
    candidate
}

fn short_hash(server: &str, tool: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(format!("{server}\0{tool}").as_bytes())
        .iter()
        .take(2)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Gives a name or description the `'static` lifetime tool definitions need. Each distinct text
/// is kept once, and only a bounded number are accepted, so a server cannot grow memory by
/// changing its descriptions.
fn intern(text: &str) -> &'static str {
    static KNOWN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    const MAX_KEPT: usize = 4096;
    let mut known = KNOWN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(found) = known.get(text) {
        return found;
    }
    if known.len() >= MAX_KEPT {
        return "(too many tools)";
    }
    let kept: &'static str = Box::leak(text.to_owned().into_boxed_str());
    known.insert(kept);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::mcp_client::tests::{helper, quick};
    use std::time::{Duration, Instant};

    fn wait_for(hub: &Hub, id: &str, done: impl Fn(&Phase) -> bool) -> Status {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = hub.statuses().into_iter().find(|status| status.id == id)
                && done(&status.phase)
            {
                return status;
            }
            assert!(Instant::now() < deadline, "{id} did not get there");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn started(hub: &Hub, mode: &str) -> String {
        let info = helper(mode);
        let id = info.id.clone();
        hub.start_with(info, Vec::new(), quick());
        wait_for(hub, &id, |phase| *phase != Phase::Starting);
        id
    }

    #[test]
    fn a_server_starts_in_the_background_and_its_tools_get_model_facing_names() {
        let hub = Hub::default();
        let id = started(&hub, "basic");
        let status = wait_for(&hub, &id, |phase| *phase == Phase::Ready);
        assert_eq!(status.tools, 7);
        let names = hub
            .definitions()
            .iter()
            .map(|tool| tool.name.to_owned())
            .collect::<Vec<_>>();
        assert!(
            names.contains(&"mcp__helper-basic__echo".to_owned()),
            "{names:?}"
        );
        let echo = hub.find("mcp__helper-basic__echo").expect("found");
        assert_eq!(echo.server, "helper-basic");
        assert_eq!(echo.tool.name, "echo");
        let definition = hub
            .definitions()
            .into_iter()
            .find(|tool| tool.name == "mcp__helper-basic__echo")
            .unwrap();
        assert!(
            definition
                .description
                .starts_with("[MCP server helper-basic] the echo tool")
        );
        assert_eq!(definition.parameters["type"], "object");
        let output = hub
            .call(
                "mcp__helper-basic__echo",
                &serde_json::json!({"text": "x"}),
                &AtomicBool::new(false),
            )
            .expect("call");
        assert_eq!(output.text, r#"echo {"text":"x"}"#);
        assert!(
            hub.call(
                "mcp__helper-basic__missing",
                &serde_json::json!({}),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        hub.stop_all();
    }

    #[test]
    fn stopping_a_server_removes_its_tools_and_a_failed_one_offers_none() {
        let hub = Hub::default();
        let id = started(&hub, "basic");
        assert!(hub.any_tools());
        hub.stop(&id);
        assert!(!hub.any_tools());
        assert!(hub.statuses().is_empty());
        let failed = started(&hub, "old");
        let status = hub.statuses().into_iter().find(|s| s.id == failed).unwrap();
        assert!(
            matches!(status.phase, Phase::Failed(ref why) if why.contains("does not support")),
            "{status:?}"
        );
        assert!(!hub.any_tools());
        assert_eq!(hub.ready(), 0);
    }

    #[test]
    fn a_server_that_dies_later_stops_offering_tools() {
        let hub = Hub::default();
        let id = started(&hub, "basic");
        let client = {
            let servers = lock(&hub.servers);
            match &servers.entries[0].state {
                State::Ready { client, .. } => client.clone(),
                _ => panic!("ready"),
            }
        };
        client.close();
        assert!(!hub.any_tools());
        let status = wait_for(&hub, &id, |phase| matches!(phase, Phase::Failed(_)));
        assert_eq!(status.tools, 1 + 6);
        let error = hub
            .call(
                "mcp__helper-basic__echo",
                &serde_json::json!({}),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(error.contains("not running"), "{error}");
    }

    #[test]
    fn restarting_replaces_the_old_server_and_a_stop_during_start_wins() {
        let hub = Hub::default();
        let info = helper("basic");
        hub.start_with(info.clone(), Vec::new(), quick());
        hub.start_with(info.clone(), Vec::new(), quick());
        assert_eq!(hub.statuses().len(), 1, "one entry per id");
        hub.stop(&info.id);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            hub.statuses().is_empty(),
            "a start that finishes after a stop is dropped"
        );
    }

    #[test]
    fn a_plugins_servers_can_be_stopped_by_prefix() {
        let hub = Hub::default();
        for (id, mode) in [
            ("kit/a", "basic"),
            ("kit/b", "basic"),
            ("kitten/a", "basic"),
        ] {
            let mut info = helper(mode);
            info.id = id.to_owned();
            hub.start_with(info, Vec::new(), quick());
        }
        hub.stop_prefix("kit/");
        let left = hub.statuses().into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(left, ["kitten/a"]);
        hub.stop_all();
    }

    #[test]
    fn tool_names_are_safe_short_and_unique() {
        let mut used = HashSet::new();
        assert_eq!(qualify("files", "read", &mut used), "mcp__files__read");
        assert_eq!(
            qualify("kit/files", "read file!", &mut used),
            "mcp__kit_files__read_file_"
        );
        // The same name twice, or two names that clean to the same text, stay distinct.
        assert_eq!(qualify("files", "read", &mut used), "mcp__files__read_2");
        assert_eq!(
            qualify("kit_files", "read_file_", &mut used),
            "mcp__kit_files__read_file__2"
        );
        let long = qualify("files", &"t".repeat(100), &mut used);
        assert!(long.len() <= MAX_NAME, "{long}");
        assert!(long.starts_with("mcp__files__ttt"), "{long}");
        let other = qualify("files", &format!("{}u", "t".repeat(99)), &mut used);
        assert_ne!(long, other);
        assert!(other.len() <= MAX_NAME);
        for name in used {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{name}"
            );
        }
    }

    #[test]
    fn interning_keeps_each_text_once() {
        let first = intern("a tool name from a test");
        let second = intern("a tool name from a test");
        assert!(std::ptr::eq(first, second));
    }
}
