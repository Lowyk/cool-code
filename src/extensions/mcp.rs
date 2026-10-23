//! MCP servers: external programs that give the model more tools. Cool Code starts a server as
//! a child process and talks to it in JSON-RPC over its standard input and output.
//!
//! A server is configured in `~/.coolcode/mcp.toml`, in a trusted project's `.coolcode/mcp.toml`
//! or in a plugin's `plugin.toml`. It runs only after the user approved exactly that command
//! (recorded with a hash, so a changed entry needs approval again), gets a small environment
//! without the user's variables, and is stopped when the program exits. The only secrets it
//! receives are the variables named in its entry, read from the OS credential store.

use crate::extensions::mods::{Approval, command_line};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Largest `mcp.toml`.
const MAX_FILE_BYTES: u64 = 32 * 1024;
/// Longest server name; it becomes part of every tool name.
pub(crate) const MAX_SERVER_NAME: usize = 24;
/// Most servers one file or plugin may list.
pub(crate) const MAX_SERVERS: usize = 16;

/// One server's entry: a `[[server]]` of `mcp.toml` or a `[[mcp]]` of `plugin.toml`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerManifest {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    /// The program: a name looked up on PATH, an absolute path, or a path inside the folder the
    /// entry belongs to.
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    /// Names of environment variables the server receives. Their values are never in a file:
    /// they are kept in the OS credential store (Settings → MCP).
    #[serde(default)]
    pub(crate) env: Vec<String>,
}

impl ServerManifest {
    pub(crate) fn validate(&self) -> Result<()> {
        if !crate::extensions::valid_name(&self.name) || self.name.len() > MAX_SERVER_NAME {
            bail!(
                "the MCP server name {:?} is not valid: use up to {MAX_SERVER_NAME} lowercase letters, digits, - and _",
                self.name
            );
        }
        if self.command.trim().is_empty() || self.command.contains('\0') {
            bail!("the MCP server {} has no command to run", self.name);
        }
        if self.description.chars().count() > 300 {
            bail!(
                "the MCP server {}'s description is longer than 300 characters",
                self.name
            );
        }
        if self.args.len() > 64 {
            bail!("the MCP server {} has more than 64 arguments", self.name);
        }
        if let Some(index) = self
            .args
            .iter()
            .position(|arg| arg.contains('\0') || arg.len() > 4096)
        {
            bail!(
                "argument {} of the MCP server {} is not valid",
                index + 1,
                self.name
            );
        }
        if self.env.len() > 16 {
            bail!("the MCP server {} lists more than 16 variables", self.name);
        }
        for (index, variable) in self.env.iter().enumerate() {
            if !valid_variable(variable) {
                bail!(
                    "the variable {variable:?} of the MCP server {} is not valid: use capital letters, digits and _",
                    self.name
                );
            }
            if self.env[..index].contains(variable) {
                bail!(
                    "the MCP server {} lists the variable {variable} twice",
                    self.name
                );
            }
        }
        Ok(())
    }

    /// The command line exactly as it will run, for showing to the user.
    pub(crate) fn command_line(&self) -> String {
        command_line(&self.command, &self.args)
    }
}

/// An environment variable name: capital letters, digits and `_`, not starting with a digit.
fn valid_variable(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Where a server's entry came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// `~/.coolcode/mcp.toml`
    User,
    /// `<project>/.coolcode/mcp.toml` of a trusted folder
    Project,
    /// A plugin's `[[mcp]]` entry, by plugin name
    Plugin(String),
}

impl Source {
    pub(crate) fn label(&self) -> String {
        match self {
            Source::User => "your mcp.toml".to_owned(),
            Source::Project => "this project".to_owned(),
            Source::Plugin(name) => format!("plugin {name}"),
        }
    }
}

/// A server found in a file or plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerInfo {
    /// `name`, `project/name` or `plugin/name`.
    pub(crate) id: String,
    pub(crate) source: Source,
    pub(crate) manifest: ServerManifest,
    /// Where it runs, and where a relative command is looked up.
    pub(crate) dir: PathBuf,
}

impl ServerInfo {
    pub(crate) fn new(source: Source, manifest: ServerManifest, dir: PathBuf) -> ServerInfo {
        let id = match &source {
            Source::User => manifest.name.clone(),
            Source::Project => format!("project/{}", manifest.name),
            Source::Plugin(plugin) => format!("{plugin}/{}", manifest.name),
        };
        ServerInfo {
            id,
            source,
            manifest,
            dir,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ServersFile {
    #[serde(default)]
    server: Vec<ServerManifest>,
}

/// Reads and checks one `mcp.toml`. A missing file is no servers.
fn read_file(path: &Path) -> Result<Vec<ServerManifest>> {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if size > MAX_FILE_BYTES {
        bail!("{} is larger than 32 KiB", path.display());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let file: ServersFile =
        toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
    if file.server.len() > MAX_SERVERS {
        bail!("{} lists more than {MAX_SERVERS} servers", path.display());
    }
    for (index, server) in file.server.iter().enumerate() {
        server.validate()?;
        if file.server[..index]
            .iter()
            .any(|other| other.name == server.name)
        {
            bail!("{} lists the server {} twice", path.display(), server.name);
        }
    }
    Ok(file.server)
}

/// Every server: those of `~/.coolcode/mcp.toml`, those of the project (when `project` is the
/// root of a trusted folder) and those of enabled plugins, and a warning for each file or plugin
/// that could not be read. Nothing here starts anything.
pub(crate) fn discover(
    dirs: &crate::extensions::Dirs,
    project: Option<&Path>,
    settings: &crate::Settings,
) -> (Vec<ServerInfo>, Vec<String>) {
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    if let (Some(dir), Some(path)) = (dirs.coolcode.as_ref(), dirs.coolcode_join("mcp.toml")) {
        match read_file(&path) {
            Ok(servers) => found.extend(
                servers
                    .into_iter()
                    .map(|server| ServerInfo::new(Source::User, server, dir.clone())),
            ),
            Err(error) => warnings.push(format!("{error:#}")),
        }
    }
    if let Some(root) = project {
        let path = root.join(".coolcode").join("mcp.toml");
        match read_file(&path) {
            Ok(servers) => found.extend(
                servers
                    .into_iter()
                    .map(|server| ServerInfo::new(Source::Project, server, root.to_path_buf())),
            ),
            Err(error) => warnings.push(format!("{error:#}")),
        }
    }
    let (plugins, _) = crate::extensions::plugins::installed(dirs, settings);
    for plugin in plugins.iter().filter(|plugin| plugin.enabled) {
        found.extend(plugin.mcp_servers());
    }
    (found, warnings)
}

/// The fingerprint an approval is recorded with: anything that changes what runs changes it.
pub(crate) fn manifest_hash(info: &ServerInfo) -> String {
    use sha2::Digest as _;
    let described = serde_json::json!({
        "id": info.id,
        "dir": info.dir.to_string_lossy(),
        "manifest": info.manifest,
    });
    sha2::Sha256::digest(described.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether the user approved this exact entry.
pub(crate) fn approval(settings: &crate::Settings, info: &ServerInfo) -> Approval {
    match settings.approved_mcp.get(&info.id) {
        None => Approval::NotApproved,
        Some(hash) if *hash == manifest_hash(info) => Approval::Approved,
        Some(_) => Approval::Changed,
    }
}

/// Approved and switched on.
pub(crate) fn should_run(settings: &crate::Settings, info: &ServerInfo) -> bool {
    approval(settings, info) == Approval::Approved && !settings.disabled_mcp.contains(&info.id)
}

/// Records the user's approval of the entry as it is now, and switches the server on.
pub(crate) fn approve(settings: &mut crate::Settings, info: &ServerInfo) {
    settings
        .approved_mcp
        .insert(info.id.clone(), manifest_hash(info));
    settings.disabled_mcp.retain(|id| *id != info.id);
}

/// Forgets what was recorded about one server.
pub(crate) fn forget(settings: &mut crate::Settings, id: &str) {
    settings.approved_mcp.remove(id);
    settings.disabled_mcp.retain(|known| known != id);
}

/// Forgets everything recorded about a plugin's servers.
pub(crate) fn forget_plugin(settings: &mut crate::Settings, plugin: &str) {
    let prefix = format!("{plugin}/");
    settings
        .approved_mcp
        .retain(|id, _| !id.starts_with(&prefix));
    settings.disabled_mcp.retain(|id| !id.starts_with(&prefix));
}

/// The credential-store entry holding one variable of one server.
fn secret_name(id: &str, variable: &str) -> String {
    format!("mcp/{id}/{variable}")
}

/// Saves the value of a server's variable in the OS credential store.
pub(crate) fn store_variable(id: &str, variable: &str, value: &str) -> Result<()> {
    crate::secrets::store(&secret_name(id, variable), value)
}

/// Removes the value of a server's variable from the OS credential store.
pub(crate) fn delete_variable(id: &str, variable: &str) -> Result<()> {
    crate::secrets::delete(&secret_name(id, variable))
}

/// Whether the credential store has a value for the variable.
pub(crate) fn has_variable(id: &str, variable: &str) -> bool {
    matches!(
        crate::secrets::load(&secret_name(id, variable)),
        Ok(Some(_))
    )
}

/// The variables the server asked for (and the user set) with their values. A variable with no
/// value is left out, so the server reports what it is missing.
pub(crate) fn variables(info: &ServerInfo) -> Vec<(String, String)> {
    info.manifest
        .env
        .iter()
        .filter_map(
            |variable| match crate::secrets::load(&secret_name(&info.id, variable)) {
                Ok(Some(value)) if !value.is_empty() => Some((variable.clone(), value)),
                _ => None,
            },
        )
        .collect()
}

/// Adds a server to `~/.coolcode/mcp.toml` (it still has to be approved before it runs).
pub(crate) fn add_user_server(
    dirs: &crate::extensions::Dirs,
    manifest: ServerManifest,
) -> Result<()> {
    manifest.validate()?;
    let path = dirs
        .coolcode_join("mcp.toml")
        .context("there is no home folder to keep mcp.toml in")?;
    let mut servers = read_file(&path)?;
    if servers.iter().any(|server| server.name == manifest.name) {
        bail!("a server named {} is already configured", manifest.name);
    }
    if servers.len() >= MAX_SERVERS {
        bail!("at most {MAX_SERVERS} servers can be configured");
    }
    servers.push(manifest);
    write_user_servers(&path, servers)
}

/// Removes a server from `~/.coolcode/mcp.toml`. Returns whether there was one.
pub(crate) fn remove_user_server(dirs: &crate::extensions::Dirs, name: &str) -> Result<bool> {
    let path = dirs
        .coolcode_join("mcp.toml")
        .context("there is no home folder to keep mcp.toml in")?;
    let mut servers = read_file(&path)?;
    let before = servers.len();
    servers.retain(|server| server.name != name);
    if servers.len() == before {
        return Ok(false);
    }
    write_user_servers(&path, servers)?;
    Ok(true)
}

fn write_user_servers(path: &Path, servers: Vec<ServerManifest>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string(&ServersFile { server: servers })?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::Dirs;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("coolcode-mcp-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("folder");
        path.canonicalize().expect("real path")
    }

    fn manifest(name: &str) -> ServerManifest {
        ServerManifest {
            name: name.to_owned(),
            description: String::new(),
            command: "npx".to_owned(),
            args: vec!["-y".to_owned(), "some-server".to_owned()],
            env: vec!["TOKEN".to_owned()],
        }
    }

    fn info(source: Source, name: &str) -> ServerInfo {
        ServerInfo::new(source, manifest(name), temp("dir"))
    }

    #[test]
    fn names_variables_and_commands_are_checked() {
        assert!(manifest("files").validate().is_ok());
        let bad = [
            ServerManifest {
                name: "Files".into(),
                ..manifest("x")
            },
            ServerManifest {
                name: "a/b".into(),
                ..manifest("x")
            },
            ServerManifest {
                name: "x".repeat(MAX_SERVER_NAME + 1),
                ..manifest("x")
            },
            ServerManifest {
                command: "  ".into(),
                ..manifest("x")
            },
            ServerManifest {
                env: vec!["lower".into()],
                ..manifest("x")
            },
            ServerManifest {
                env: vec!["1ST".into()],
                ..manifest("x")
            },
            ServerManifest {
                env: vec!["A".into(), "A".into()],
                ..manifest("x")
            },
            ServerManifest {
                args: vec!["a\0b".into()],
                ..manifest("x")
            },
            ServerManifest {
                description: "d".repeat(301),
                ..manifest("x")
            },
        ];
        for server in bad {
            assert!(server.validate().is_err(), "{server:?}");
        }
        assert_eq!(manifest("x").command_line(), "npx -y some-server");
    }

    #[test]
    fn ids_say_where_a_server_came_from() {
        assert_eq!(info(Source::User, "files").id, "files");
        assert_eq!(info(Source::Project, "files").id, "project/files");
        assert_eq!(info(Source::Plugin("kit".into()), "files").id, "kit/files");
    }

    #[test]
    fn an_approval_holds_only_for_the_entry_that_was_approved() {
        let mut settings = crate::Settings::default();
        let mut server = info(Source::User, "files");
        assert_eq!(approval(&settings, &server), Approval::NotApproved);
        assert!(!should_run(&settings, &server));
        approve(&mut settings, &server);
        assert_eq!(approval(&settings, &server), Approval::Approved);
        assert!(should_run(&settings, &server));
        for change in [
            |s: &mut ServerInfo| s.manifest.command = "other".into(),
            |s: &mut ServerInfo| s.manifest.args.push("--extra".into()),
            |s: &mut ServerInfo| s.manifest.env.push("MORE".into()),
            |s: &mut ServerInfo| s.dir = std::env::temp_dir(),
        ] {
            let before = server.clone();
            change(&mut server);
            assert_eq!(approval(&settings, &server), Approval::Changed);
            assert!(!should_run(&settings, &server), "{server:?}");
            server = before;
        }
        settings.disabled_mcp.push("files".into());
        assert!(!should_run(&settings, &server), "switched off");
        approve(&mut settings, &server);
        assert!(should_run(&settings, &server), "approving switches it on");
    }

    #[test]
    fn forgetting_a_plugin_forgets_only_its_servers() {
        let mut settings = crate::Settings::default();
        for id in ["kit/a", "kit/b", "kitten/a", "kit"] {
            settings.approved_mcp.insert(id.into(), "hash".into());
            settings.disabled_mcp.push(id.into());
        }
        forget_plugin(&mut settings, "kit");
        let left = settings.approved_mcp.keys().cloned().collect::<Vec<_>>();
        assert_eq!(left, ["kit", "kitten/a"]);
        assert_eq!(settings.disabled_mcp, ["kitten/a", "kit"]);
    }

    #[test]
    fn servers_come_from_the_user_file_and_only_a_trusted_project() {
        let home = temp("home");
        let dirs = Dirs::under(&home);
        std::fs::create_dir_all(home.join(".coolcode")).unwrap();
        std::fs::write(
            home.join(".coolcode/mcp.toml"),
            "[[server]]\nname = \"files\"\ncommand = \"npx\"\nargs = [\"-y\", \"fs\"]\n",
        )
        .unwrap();
        let project = temp("project");
        std::fs::create_dir_all(project.join(".coolcode")).unwrap();
        std::fs::write(
            project.join(".coolcode/mcp.toml"),
            "[[server]]\nname = \"db\"\ncommand = \"./db-server\"\n",
        )
        .unwrap();
        let settings = crate::Settings::default();
        let (untrusted, warnings) = discover(&dirs, None, &settings);
        assert_eq!(
            untrusted.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["files"],
            "an untrusted folder's file is never read: {warnings:?}"
        );
        let (trusted, _) = discover(&dirs, Some(&project), &settings);
        let ids = trusted.iter().map(|s| s.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, ["files", "project/db"]);
        assert_eq!(trusted[1].dir, project);
        assert_eq!(trusted[1].source, Source::Project);
    }

    #[test]
    fn a_bad_file_is_a_warning_and_the_other_servers_still_load() {
        let home = temp("home");
        let dirs = Dirs::under(&home);
        std::fs::create_dir_all(home.join(".coolcode")).unwrap();
        std::fs::write(
            home.join(".coolcode/mcp.toml"),
            "[[server]]\nname = \"Bad Name\"\ncommand = \"x\"\n",
        )
        .unwrap();
        let project = temp("project");
        std::fs::create_dir_all(project.join(".coolcode")).unwrap();
        std::fs::write(
            project.join(".coolcode/mcp.toml"),
            "[[server]]\nname = \"ok\"\ncommand = \"x\"\n",
        )
        .unwrap();
        let (found, warnings) = discover(&dirs, Some(&project), &crate::Settings::default());
        assert_eq!(found.len(), 1);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("not valid"), "{warnings:?}");
    }

    #[test]
    fn unknown_keys_and_duplicate_names_are_refused() {
        let dir = temp("file");
        let path = dir.join("mcp.toml");
        std::fs::write(
            &path,
            "[[server]]\nname = \"a\"\ncommand = \"x\"\nauto_approve = true\n",
        )
        .unwrap();
        assert!(read_file(&path).is_err(), "unknown keys cannot sneak in");
        std::fs::write(&path, "[[server]]\nname = \"a\"\ncommand = \"x\"\n[[server]]\nname = \"a\"\ncommand = \"y\"\n").unwrap();
        assert!(format!("{:#}", read_file(&path).unwrap_err()).contains("twice"));
        assert!(read_file(&dir.join("missing.toml")).unwrap().is_empty());
    }

    #[test]
    fn adding_and_removing_a_user_server_rewrites_the_file() {
        let home = temp("home");
        let dirs = Dirs::under(&home);
        add_user_server(&dirs, manifest("files")).unwrap();
        add_user_server(&dirs, manifest("db")).unwrap();
        assert!(
            add_user_server(&dirs, manifest("db")).is_err(),
            "no duplicates"
        );
        assert!(
            add_user_server(
                &dirs,
                ServerManifest {
                    name: "Bad".into(),
                    ..manifest("x")
                }
            )
            .is_err()
        );
        let (found, _) = discover(&dirs, None, &crate::Settings::default());
        assert_eq!(found.len(), 2);
        assert!(remove_user_server(&dirs, "files").unwrap());
        assert!(!remove_user_server(&dirs, "files").unwrap());
        let (found, _) = discover(&dirs, None, &crate::Settings::default());
        assert_eq!(
            found.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["db"]
        );
    }
}
