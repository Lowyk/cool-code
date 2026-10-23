//! MCP servers in the interface: starting the approved ones, `/mcp`, and the count in the
//! status line. Approving a server happens in a review that shows its exact command (Settings →
//! Plugins, or `/mcp approve <name>`); nothing starts before that.

use crate::extensions::mcp::{self, ServerInfo, ServerManifest, Source};
use crate::extensions::mcp_hub::{Phase, hub};
use crate::extensions::mods::Approval;
use crate::tui::state::App;
use crate::write_settings;
use anyhow::Result;
use ratatui::style::Color;

const USAGE: &str = "Usage: /mcp [list] | /mcp add <name> <command> [arguments…] | /mcp approve <name> | /mcp enable <name> | /mcp disable <name> | /mcp restart <name> | /mcp secret <name> | /mcp remove <name>";

impl App {
    /// Every configured server (those of the project only in a trusted folder) and a warning for
    /// each file that could not be read.
    pub(in crate::tui) fn mcp_servers(&self) -> (Vec<ServerInfo>, Vec<String>) {
        let project = project_root(
            self.workspace_trusted,
            std::env::current_dir()
                .and_then(|path| path.canonicalize())
                .ok(),
        );
        mcp::discover(&self.extensions.dirs, project.as_deref(), &self.settings)
    }

    /// Makes the running servers match the approved, switched-on ones: stops the others,
    /// restarts one whose entry changed, and starts the new ones. Does nothing until the
    /// interface itself is running.
    pub(in crate::tui) fn sync_mcp(&mut self) {
        if !self.extensions.mods_active {
            return;
        }
        let (found, _) = self.mcp_servers();
        let wanted = found
            .into_iter()
            .filter(|info| mcp::should_run(&self.settings, info))
            .collect::<Vec<_>>();
        let hub = hub();
        let known = self
            .extensions
            .mcp_started
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in known {
            let current = wanted
                .iter()
                .find(|info| info.id == id)
                .map(mcp::manifest_hash);
            if current.as_ref() != self.extensions.mcp_started.get(&id) {
                hub.stop(&id);
                self.extensions.mcp_started.remove(&id);
            }
        }
        for info in wanted {
            if !self.extensions.mcp_started.contains_key(&info.id) {
                self.extensions
                    .mcp_started
                    .insert(info.id.clone(), mcp::manifest_hash(&info));
                let variables = mcp::variables(&info);
                hub.start(info, variables);
            }
        }
    }

    /// Stops every server, when the program exits.
    pub(in crate::tui) fn stop_mcp(&mut self) {
        self.extensions.mcp_started.clear();
        hub().stop_all();
    }

    /// Starts one server again (after its secrets changed, or because it failed).
    pub(in crate::tui) fn restart_mcp(&mut self, id: &str) {
        hub().stop(id);
        self.extensions.mcp_started.remove(id);
        self.sync_mcp();
    }

    /// How many servers are up out of those that are switched on: `MCP 2/3`. Nothing when none
    /// is configured.
    pub(in crate::tui) fn mcp_status(&self) -> Option<(String, bool)> {
        if self.extensions.mcp_started.is_empty() {
            return None;
        }
        let ready = hub().ready();
        let total = self.extensions.mcp_started.len();
        Some((format!("MCP {ready}/{total}"), ready < total))
    }

    /// `/mcp ...`: returns what to show in the conversation.
    pub(in crate::tui) fn mcp_command(&mut self, arguments: &str) -> Result<String> {
        let mut words = arguments.split_whitespace();
        let word = words.next().unwrap_or("");
        let rest = words.collect::<Vec<_>>();
        match (word, rest.as_slice()) {
            ("" | "list", []) => Ok(self.mcp_list()),
            ("add", [name, command, args @ ..]) => {
                let manifest = ServerManifest {
                    name: (*name).to_owned(),
                    description: String::new(),
                    command: (*command).to_owned(),
                    args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                    env: Vec::new(),
                };
                Ok(
                    match mcp::add_user_server(&self.extensions.dirs, manifest) {
                        Ok(()) => format!(
                            "Added the MCP server {name}. It does not run until you approve it: /mcp approve {name}. Servers that need secrets are set up in mcp.toml (env = [\"NAME\"]) and Settings → Plugins."
                        ),
                        Err(error) => format!("Could not add the server: {error:#}"),
                    },
                )
            }
            ("approve", [id]) => Ok(self.open_mcp_review(id)),
            ("secret", [id]) => Ok(self.open_mcp_secrets(id)),
            ("enable" | "disable", [id]) => self.switch_mcp(id, word == "enable"),
            ("restart", [id]) => Ok(match self.find_mcp(id) {
                Some(info) if mcp::should_run(&self.settings, &info) => {
                    self.restart_mcp(id);
                    format!("Restarting the MCP server {id}.")
                }
                Some(_) => {
                    format!("The MCP server {id} is not approved and on, so it is not running.")
                }
                None => format!("There is no MCP server named {id}."),
            }),
            ("remove", [name]) => self.remove_mcp(name),
            _ => Ok(USAGE.to_owned()),
        }
    }

    /// The credential-store entries of a plugin's servers as (server id, variable), listed
    /// before the plugin is deleted because afterwards nothing says what they were.
    pub(in crate::tui) fn plugin_secrets(&self, plugin: &str) -> Vec<(String, String)> {
        let (installed, _) =
            crate::extensions::plugins::installed(&self.extensions.dirs, &self.settings);
        installed
            .into_iter()
            .filter(|installed| installed.manifest.name == plugin)
            .flat_map(|installed| installed.mcp_servers())
            .flat_map(|info| {
                let id = info.id.clone();
                info.manifest
                    .env
                    .into_iter()
                    .map(move |variable| (id.clone(), variable))
            })
            .collect()
    }

    fn find_mcp(&self, id: &str) -> Option<ServerInfo> {
        self.mcp_servers().0.into_iter().find(|info| info.id == id)
    }

    fn mcp_list(&self) -> String {
        let (servers, warnings) = self.mcp_servers();
        let mut text = if servers.is_empty() {
            "No MCP servers are configured. Add one with /mcp add <name> <command> [arguments…], or list it in ~/.coolcode/mcp.toml.".to_owned()
        } else {
            "MCP servers (approve, switch on or off and set secrets in Settings > Plugins):"
                .to_owned()
        };
        let statuses = hub().statuses();
        for info in &servers {
            let (state, _) = mcp_state(self, info, &statuses);
            text.push_str(&format!(
                "\n  {} · {state} · from {} · {}",
                info.id,
                info.source.label(),
                info.manifest.command_line()
            ));
        }
        for warning in warnings {
            text.push_str(&format!("\n  Skipped {warning}"));
        }
        text
    }

    /// Opens the review of a server in Settings → Plugins.
    fn open_mcp_review(&mut self, id: &str) -> String {
        if self.find_mcp(id).is_none() {
            return format!("There is no MCP server named {id}.");
        }
        self.open_settings(crate::tui::settings::Section::Plugins);
        if let Some(view) = self.settings_view.as_mut() {
            view.focus = crate::tui::settings::Focus::Content;
            view.plugin_confirm = Some(crate::tui::settings::plugins::PluginConfirm::ApproveMcp(
                id.to_owned(),
            ));
        }
        format!("Review the MCP server {id} and press y to approve it.")
    }

    /// Opens the entry for a server's secrets in Settings → Plugins.
    fn open_mcp_secrets(&mut self, id: &str) -> String {
        let Some(info) = self.find_mcp(id) else {
            return format!("There is no MCP server named {id}.");
        };
        if info.manifest.env.is_empty() {
            return format!("The MCP server {id} asks for no secrets (its entry has no env list).");
        }
        self.open_settings(crate::tui::settings::Section::Plugins);
        if let Some(view) = self.settings_view.as_mut() {
            view.focus = crate::tui::settings::Focus::Content;
            view.mcp_secret = Some(crate::tui::settings::plugins::SecretInput::new(&info));
        }
        format!("Type the value of each variable of {id}; Enter saves it.")
    }

    fn switch_mcp(&mut self, id: &str, on: bool) -> Result<String> {
        let Some(info) = self.find_mcp(id) else {
            return Ok(format!("There is no MCP server named {id}."));
        };
        if mcp::approval(&self.settings, &info) != Approval::Approved {
            return Ok(format!(
                "The MCP server {id} is not approved yet: /mcp approve {id}."
            ));
        }
        if on {
            self.settings.disabled_mcp.retain(|other| other != id);
        } else if !self.settings.disabled_mcp.iter().any(|other| other == id) {
            self.settings.disabled_mcp.push(id.to_owned());
        }
        write_settings(&self.settings)?;
        self.sync_mcp();
        Ok(format!(
            "The MCP server {id} is {}.",
            if on { "on" } else { "off" }
        ))
    }

    /// Removes a server from `~/.coolcode/mcp.toml`, with its approval and secrets.
    fn remove_mcp(&mut self, name: &str) -> Result<String> {
        let Some(info) = self.find_mcp(name) else {
            return Ok(format!("There is no MCP server named {name}."));
        };
        if info.source != Source::User {
            return Ok(format!(
                "The MCP server {name} comes from {}: remove it there.",
                info.source.label()
            ));
        }
        mcp::remove_user_server(&self.extensions.dirs, name)?;
        for variable in &info.manifest.env {
            let _ = mcp::delete_variable(&info.id, variable);
        }
        mcp::forget(&mut self.settings, &info.id);
        write_settings(&self.settings)?;
        self.sync_mcp();
        Ok(format!("Removed the MCP server {name}."))
    }
}

/// Deletes credential-store entries listed by [`App::plugin_secrets`].
pub(in crate::tui) fn delete_secrets(secrets: &[(String, String)]) {
    for (id, variable) in secrets {
        let _ = mcp::delete_variable(id, variable);
    }
}

/// The folder whose `.coolcode/mcp.toml` may be read: only a folder the user trusts.
fn project_root(trusted: bool, folder: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    folder.filter(|_| trusted)
}

/// How a server stands, as shown in lists: its approval, then what the hub says.
pub(in crate::tui) fn mcp_state(
    app: &App,
    info: &ServerInfo,
    statuses: &[crate::extensions::mcp_hub::Status],
) -> (String, Color) {
    let amber = Color::Rgb(255, 197, 92);
    match mcp::approval(&app.settings, info) {
        Approval::NotApproved => ("needs approval".to_owned(), amber),
        Approval::Changed => ("changed, approve again".to_owned(), amber),
        Approval::Approved if app.settings.disabled_mcp.contains(&info.id) => {
            ("off".to_owned(), Color::Gray)
        }
        Approval::Approved => match statuses.iter().find(|status| status.id == info.id) {
            None => ("on, not started yet".to_owned(), Color::Gray),
            Some(status) => match &status.phase {
                Phase::Starting => ("starting".to_owned(), Color::Gray),
                Phase::Ready => (
                    format!("running, {} tools", status.tools),
                    Color::Rgb(110, 220, 130),
                ),
                Phase::Failed(reason) => (
                    format!("failed: {}", crate::extensions::mods::sanitize(reason, 60)),
                    Color::Rgb(235, 80, 80),
                ),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::Dirs;
    use crate::extensions::mcp_client::tests::helper;
    use crate::extensions::mcp_hub::use_private_hub;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn home() -> PathBuf {
        let path = std::env::temp_dir().join(format!("coolcode-tui-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("home");
        path
    }

    fn app_in(home: &std::path::Path) -> App {
        use_private_hub();
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = true;
        app.extensions.dirs = Dirs::under(home);
        app.extensions.mods_active = true;
        app
    }

    /// Writes the test server into the user's mcp.toml under `name`.
    fn configure(app: &App, name: &str) -> ServerInfo {
        let mut manifest = helper("basic").manifest;
        manifest.name = name.to_owned();
        mcp::add_user_server(&app.extensions.dirs, manifest).expect("added");
        app.mcp_servers()
            .0
            .into_iter()
            .find(|info| info.id == name)
            .expect("found")
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn with_nothing_configured_the_command_says_how_to_add_a_server() {
        let mut app = app_in(&home());
        let said = app.mcp_command("").unwrap();
        assert!(said.contains("No MCP servers"), "{said}");
        assert!(said.contains("/mcp add"), "{said}");
        assert!(app.mcp_status().is_none());
        assert!(
            app.mcp_command("frobnicate")
                .unwrap()
                .contains("Usage: /mcp")
        );
    }

    #[test]
    fn adding_a_server_writes_the_file_but_nothing_runs_until_it_is_approved() {
        let mut app = app_in(&home());
        let said = app.mcp_command("add files npx -y some-server").unwrap();
        assert!(said.contains("does not run until you approve"), "{said}");
        let list = app.mcp_command("list").unwrap();
        assert!(list.contains("files · needs approval"), "{list}");
        assert!(list.contains("npx -y some-server"), "{list}");
        assert!(
            app.mcp_command("add files npx")
                .unwrap()
                .contains("already configured")
        );
        assert!(
            app.mcp_command("add Bad npx")
                .unwrap()
                .contains("not valid")
        );
        app.sync_mcp();
        assert!(
            hub().statuses().is_empty(),
            "an unapproved server never starts"
        );
    }

    #[test]
    fn an_approved_server_starts_and_stops_when_it_is_switched_off_or_changes() {
        let mut app = app_in(&home());
        let info = configure(&app, "files");
        mcp::approve(&mut app.settings, &info);
        app.sync_mcp();
        wait_until("the server starts", || hub().ready() == 1);
        assert_eq!(app.mcp_status(), Some(("MCP 1/1".to_owned(), false)));
        assert!(
            app.mcp_command("list")
                .unwrap()
                .contains("running, 7 tools")
        );
        // Switched off: it stops.
        assert!(app.mcp_command("disable files").unwrap().contains("is off"));
        assert!(hub().statuses().is_empty());
        assert!(app.mcp_command("enable files").unwrap().contains("is on"));
        wait_until("it starts again", || hub().ready() == 1);
        // Its entry changes on disk: the old approval no longer holds and it stops.
        let mut changed = info.manifest.clone();
        changed.args.push("--extra".to_owned());
        mcp::remove_user_server(&app.extensions.dirs, "files").unwrap();
        mcp::add_user_server(&app.extensions.dirs, changed).unwrap();
        app.sync_mcp();
        assert!(
            hub().statuses().is_empty(),
            "a changed entry must be approved again"
        );
        assert!(
            app.mcp_command("list")
                .unwrap()
                .contains("changed, approve again")
        );
        app.stop_mcp();
        assert!(hub().statuses().is_empty());
    }

    #[test]
    fn a_projects_servers_are_read_only_in_a_trusted_folder() {
        let folder = Some(PathBuf::from("/work/app"));
        assert_eq!(project_root(true, folder.clone()), folder);
        assert_eq!(project_root(false, folder), None);
        assert_eq!(project_root(true, None), None);
    }

    #[test]
    fn approve_opens_the_review_of_that_server() {
        let mut app = app_in(&home());
        configure(&app, "files");
        let said = app.mcp_command("approve files").unwrap();
        assert!(said.contains("press y"), "{said}");
        let view = app.settings_view.as_ref().expect("settings open");
        assert_eq!(
            view.plugin_confirm,
            Some(crate::tui::settings::plugins::PluginConfirm::ApproveMcp(
                "files".into()
            ))
        );
        assert!(
            app.mcp_command("approve nothing")
                .unwrap()
                .contains("no MCP server named")
        );
        assert!(
            app.settings.approved_mcp.is_empty(),
            "opening the review approves nothing"
        );
    }

    #[test]
    fn removing_a_user_server_forgets_its_approval_and_stops_it() {
        let mut app = app_in(&home());
        let info = configure(&app, "files");
        mcp::approve(&mut app.settings, &info);
        app.sync_mcp();
        wait_until("it starts", || hub().ready() == 1);
        assert!(app.mcp_command("remove files").unwrap().contains("Removed"));
        assert!(app.settings.approved_mcp.is_empty());
        assert!(hub().statuses().is_empty());
        assert!(app.mcp_servers().0.is_empty());
        assert!(
            app.mcp_command("remove files")
                .unwrap()
                .contains("no MCP server named")
        );
    }
}
