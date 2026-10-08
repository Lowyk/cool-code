// Test fixtures start from Default and override a few fields; that reads better than struct updates.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

mod agent;
mod chatgpt_auth;
mod context;
mod effort_support;
mod endpoints;
mod guard;
mod headless;
mod imagegen;
mod login_page;
mod policy;
mod projects;
mod prompt;
mod provider;
mod responses;
mod secrets;
mod session;
mod stats;
mod stream;
#[cfg(test)]
mod testutil;
mod tools;
mod tui;
mod workflow;

#[derive(Debug, Parser)]
#[command(
    name = "coolcode",
    version,
    about = "Cool Code: an AI coding assistant for your terminal"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Choose an earlier session in this folder to continue.
    #[arg(long, conflicts_with = "latest")]
    resume: bool,
    /// Continue the most recent session in this folder.
    #[arg(long)]
    latest: bool,
    /// With --resume or --latest, consider sessions from every folder.
    #[arg(long)]
    all_folders: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create the user configuration file if it does not exist.
    Init,
    /// Display or update local settings.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Run one turn without the interface and print the answer (for scripts and CI).
    ///
    /// The answer goes to standard output and progress to standard error. Approvals cannot be
    /// asked for, so they are declined: pick a permission mode that fits the job.
    Run {
        /// What to ask. Read from standard input when omitted or `-`.
        prompt: Vec<String>,
        /// Use this model on the active provider.
        #[arg(long)]
        model: Option<String>,
        /// Permission mode: plan, auto, accept-edits, accept-minimal or accept-everything.
        #[arg(long)]
        mode: Option<String>,
        /// Effort: low, medium, high, xhigh, max, super or ultimate (the last two need Dynamic
        /// workflows unlocked).
        #[arg(long)]
        effort: Option<Effort>,
        /// Treat the current folder as trusted for this run only.
        #[arg(long)]
        trust: bool,
        /// Print one JSON object per line instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show or set the reasoning/workflow effort level.
    Effort {
        /// Effort to select: low, medium, high, max, xhigh, super, or ultimate.
        level: Option<Effort>,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the effective settings (secrets are never displayed).
    Show,
    /// Update provider connection settings.
    Set {
        /// Provider adapter name: openai-compatible, openai, or groq.
        #[arg(long)]
        provider: Option<String>,
        /// Model identifier.
        #[arg(long)]
        model: Option<String>,
        /// API root URL, such as https://api.openai.com/v1.
        #[arg(long)]
        base_url: Option<String>,
        /// Environment variable containing the API key.
        #[arg(long)]
        api_key_env: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Effort {
    Low,
    Medium,
    High,
    Max,
    #[value(name = "xhigh")]
    #[serde(rename = "xhigh")]
    XHigh,
    Super,
    #[value(alias = "extreme")]
    #[serde(alias = "extreme")]
    Ultimate,
}

impl Effort {
    fn description(self) -> &'static str {
        match self {
            Self::Low => "provider's low effort; no workflow orchestration",
            Self::Medium => "provider's medium effort; no workflow orchestration",
            Self::High => "provider's high effort; no workflow orchestration",
            Self::XHigh => "provider's xhigh effort; no workflow orchestration",
            Self::Max => "provider's maximum normal effort",
            Self::Super => "xhigh effort with dynamic workflows and subagents",
            Self::Ultimate => {
                "max effort with dynamic workflows and subagents; potentially expensive"
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Settings {
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    providers: Vec<ProviderProfile>,
    active_provider_id: Option<String>,
    default_provider_id: Option<String>,
    model_chains: Vec<ModelChain>,
    active_chain_id: Option<String>,
    privacy_acknowledged: Vec<String>,
    privacy_image_acknowledged: Vec<String>,
    #[serde(alias = "extreme_acknowledged")]
    ultimate_acknowledged: bool,
    background_animation: bool,
    theme: ThemeId,
    /// Keep the animated backdrop behind the conversation, not only on the welcome screen.
    backdrop_in_chat: bool,
    /// Draw the backdrop dimmer behind the conversation so text stays easy to read.
    dim_backdrop_in_chat: bool,
    pulse: PulseMode,
    motion_prompt_answered: bool,
    max_tool_rounds: usize,
    /// Usage stats are recorded locally only after the user opts in.
    stats_enabled: bool,
    stats_prompt_answered: bool,
    /// Save conversations to disk so they can be resumed. Off until the user opts in.
    sessions_enabled: bool,
    sessions_prompt_answered: bool,
    theme_prompt_answered: bool,
    /// Load `CLAUDE.md` from project folders (each project can override this).
    default_load_claude_md: bool,
    /// Load `AGENTS.md` from project folders (each project can override this).
    default_load_agents_md: bool,
    /// Load the user's own `~/.claude/CLAUDE.md` in every project.
    load_global_claude_md: bool,
    instructions_prompt_answered: bool,
    /// Unlocks the Super and Ultimate effort tiers (and workflows on lower levels). Off by
    /// default because workflows can spend many times more tokens.
    dynamic_workflows: bool,
    /// Run workflows on the Low, Medium and High levels too (only while workflows are unlocked).
    workflows: bool,
    /// Keep the effort name in the status line animated instead of fading it after a change.
    effort_always_animated: bool,
    /// Draw the interface light instead of dark.
    light_mode: bool,
    /// Warn when the active provider reports that its balance or usage limits are running low.
    usage_warnings: bool,
    /// Condense the older conversation into a summary before it fills the model's context window.
    auto_compact: bool,
    /// The models Auto mode asks whether an action is safe, in the order they are tried. Auto
    /// mode is unavailable until at least one can judge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    auto_guards: Vec<guard::AutoGuard>,
    /// Allow `@` references to files outside the project folder (each one is still confirmed).
    outside_files: bool,
    /// Skip that confirmation, except for paths that may hold secrets. Hidden until unlocked.
    outside_files_no_prompt: bool,
    /// The image API `generate_image` uses (its key is in the credential store). Off when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image_generation: Option<imagegen::ImageConfig>,
    effort: Effort,
    permission_mode: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum PulseMode {
    Off,
    #[default]
    Words,
    Characters,
}

/// The look of the interface: accent colors, panel colors, and the animated backdrop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ThemeId {
    #[default]
    Cool,
    Galaxy,
    GalaxyVoid,
    Sakura,
    Mint,
    Autumn,
    Retro,
    Synthwave,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ProviderProfile {
    id: String,
    name: String,
    adapter: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    models: Vec<ModelProfile>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    auto_switch: bool,
    #[serde(default)]
    base_url: Option<String>,
    /// Optional endpoint listing the provider's models (same host as the base URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    models_url: Option<String>,
    /// Optional endpoint reporting usage limits or balance (same host as the base URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limits_url: Option<String>,
    /// Metadata the models endpoint reported, keyed by model ID.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    model_info: std::collections::BTreeMap<String, ModelInfo>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct ModelInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelProfile {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelChain {
    id: String,
    alias: String,
    members: Vec<ChainModel>,
    #[serde(default)]
    activate_on_select: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ChainModel {
    provider_id: String,
    model_id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: Some("openai-compatible".to_owned()),
            model: None,
            base_url: None,
            api_key_env: None,
            providers: Vec::new(),
            active_provider_id: None,
            default_provider_id: None,
            model_chains: Vec::new(),
            active_chain_id: None,
            privacy_acknowledged: Vec::new(),
            privacy_image_acknowledged: Vec::new(),
            ultimate_acknowledged: false,
            background_animation: true,
            theme: ThemeId::Cool,
            backdrop_in_chat: false,
            dim_backdrop_in_chat: true,
            pulse: PulseMode::Words,
            motion_prompt_answered: false,
            max_tool_rounds: 40,
            stats_enabled: false,
            stats_prompt_answered: false,
            sessions_enabled: false,
            sessions_prompt_answered: false,
            theme_prompt_answered: false,
            default_load_claude_md: false,
            default_load_agents_md: false,
            load_global_claude_md: false,
            instructions_prompt_answered: false,
            dynamic_workflows: false,
            workflows: false,
            effort_always_animated: false,
            light_mode: false,
            usage_warnings: true,
            auto_compact: true,
            auto_guards: Vec::new(),
            outside_files: false,
            outside_files_no_prompt: false,
            image_generation: None,
            effort: Effort::High,
            permission_mode: "plan".to_owned(),
        }
    }
}

#[cfg(not(test))]
fn settings_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not locate the home directory")?;
    Ok(home.join(".coolcode").join("config.toml"))
}

#[cfg(not(test))]
fn legacy_settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("harness").join("config.toml"))
}

#[cfg(test)]
fn legacy_settings_path() -> Option<PathBuf> {
    None
}

static SETTINGS_MIGRATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn settings_were_migrated() -> bool {
    SETTINGS_MIGRATED.load(std::sync::atomic::Ordering::Relaxed)
}

// Tests exercise code paths that persist settings; keep them away from the user's config.
#[cfg(test)]
fn settings_path() -> Result<PathBuf> {
    // One file per test thread (every test runs on its own), so tests that save and reload
    // settings cannot overwrite each other; the start time keeps reused process ids apart.
    static RUN: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    let run = RUN.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    });
    let thread = format!("{:?}", std::thread::current().id()).replace(['(', ')'], "-");
    Ok(std::env::temp_dir()
        .join(format!("harness-test-{}-{run}", std::process::id()))
        .join(thread)
        .join("config.toml"))
}

/// Copies the legacy config to the new location once; the legacy file is kept as a backup.
fn migrate_settings(new: &std::path::Path, legacy: &std::path::Path) -> Result<bool> {
    if new.exists() || !legacy.exists() {
        return Ok(false);
    }
    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::copy(legacy, new)
        .with_context(|| format!("copying {} to {}", legacy.display(), new.display()))?;
    Ok(true)
}

fn read_settings() -> Result<Settings> {
    let path = settings_path()?;
    if let Some(legacy) = legacy_settings_path()
        && migrate_settings(&path, &legacy)?
    {
        SETTINGS_MIGRATED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn write_settings(settings: &Settings) -> Result<()> {
    let path = settings_path()?;
    let parent = path
        .parent()
        .context("config path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let text = toml::to_string_pretty(settings).context("serializing settings")?;
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.all_folders && !(cli.resume || cli.latest) {
        anyhow::bail!("--all-folders only applies together with --resume or --latest");
    }
    let Some(command) = cli.command else {
        let resume = if cli.resume {
            Some(tui::sessions::Resume::Pick {
                all_folders: cli.all_folders,
            })
        } else if cli.latest {
            Some(tui::sessions::Resume::Latest {
                all_folders: cli.all_folders,
            })
        } else {
            None
        };
        return tui::run(resume);
    };
    match command {
        Command::Run {
            prompt,
            model,
            mode,
            effort,
            trust,
            json,
        } => headless::run(headless::Options {
            prompt: prompt.join(" "),
            model,
            mode,
            effort,
            trust,
            json,
        })?,
        Command::Init => {
            let path = settings_path()?;
            if path.exists() {
                println!("Settings already exist at {}", path.display());
            } else {
                write_settings(&Settings::default())?;
                println!("Created settings at {}", path.display());
            }
        }
        Command::Config { command } => match command {
            ConfigCommand::Show => {
                let settings = read_settings()?;
                println!(
                    "provider       = {}",
                    settings.provider.as_deref().unwrap_or("openai-compatible")
                );
                println!(
                    "model          = {}",
                    settings.model.as_deref().unwrap_or("(not set)")
                );
                println!(
                    "base_url       = {}",
                    settings.base_url.as_deref().unwrap_or("(provider default)")
                );
                println!(
                    "api_key_env    = {}",
                    settings
                        .api_key_env
                        .as_deref()
                        .unwrap_or("(provider default)")
                );
                println!("effort         = {:?}", settings.effort);
                println!("permission_mode= {}", settings.permission_mode);
                println!("config         = {}", settings_path()?.display());
            }
            ConfigCommand::Set {
                provider,
                model,
                base_url,
                api_key_env,
            } => {
                if provider.is_none()
                    && model.is_none()
                    && base_url.is_none()
                    && api_key_env.is_none()
                {
                    anyhow::bail!("provide at least one setting to update");
                }
                let mut settings = read_settings()?;
                settings.active_provider_id = None;
                if let Some(value) = provider {
                    settings.provider = Some(value);
                }
                if let Some(value) = model {
                    settings.model = Some(value);
                }
                if let Some(value) = base_url {
                    settings.base_url = Some(value.trim_end_matches('/').to_owned());
                }
                if let Some(value) = api_key_env {
                    settings.api_key_env = Some(value);
                }
                write_settings(&settings)?;
                println!("Updated settings at {}", settings_path()?.display());
            }
        },
        Command::Effort { level } => {
            let mut settings = read_settings()?;
            if let Some(level) = level {
                if level.is_workflow_tier() && !settings.dynamic_workflows {
                    println!(
                        "{level:?} is locked. Turn on Dynamic workflows in Settings → General first."
                    );
                    return Ok(());
                }
                if matches!(level, Effort::Ultimate) {
                    eprintln!(
                        "Warning: Ultimate may use substantially more tokens and incur higher cost."
                    );
                    eprintln!(
                        "It also runs subagents and reviews, which can use many times more tokens."
                    );
                    eprint!("Set Ultimate effort anyway? [y/N] ");
                    io::stderr().flush().context("flushing warning")?;
                    let mut answer = String::new();
                    io::stdin()
                        .read_line(&mut answer)
                        .context("reading confirmation")?;
                    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                        println!("Effort unchanged.");
                        return Ok(());
                    }
                }
                settings.effort = level;
                write_settings(&settings)?;
                println!("Effort set to {:?}: {}", level, level.description());
            } else {
                println!("Current effort: {:?}", settings.effort);
                println!("Available levels:");
                for effort in [
                    Effort::Low,
                    Effort::Medium,
                    Effort::High,
                    Effort::XHigh,
                    Effort::Max,
                    Effort::Super,
                    Effort::Ultimate,
                ] {
                    println!(
                        "  {:<8} {}",
                        format!("{:?}", effort).to_lowercase(),
                        effort.description()
                    );
                }
                println!("Use `coolcode effort <level>` to select one.");
            }
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_written_before_the_rename_still_load() {
        let old: Settings =
            toml::from_str("effort = \"extreme\"\nextreme_acknowledged = true\n").expect("parse");
        assert_eq!(old.effort, Effort::Ultimate);
        assert!(old.ultimate_acknowledged);
        let current: Settings = toml::from_str("effort = \"ultimate\"\n").expect("parse");
        assert_eq!(current.effort, Effort::Ultimate);
        let written = toml::to_string(&old).expect("write");
        assert!(written.contains("ultimate"), "{written}");
    }

    #[test]
    fn the_program_is_called_coolcode() {
        use clap::CommandFactory;
        assert_eq!(Cli::command().get_name(), "coolcode");
        assert_eq!(env!("CARGO_PKG_NAME"), "coolcode");
    }

    #[test]
    fn the_run_command_takes_a_prompt_and_its_options() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "coolcode",
            "run",
            "fix",
            "the",
            "bug",
            "--mode",
            "accept-edits",
            "--effort",
            "high",
            "--trust",
            "--json",
        ])
        .expect("parse");
        match cli.command {
            Some(Command::Run {
                prompt,
                mode,
                effort,
                trust,
                json,
                ..
            }) => {
                assert_eq!(prompt.join(" "), "fix the bug");
                assert_eq!(mode.as_deref(), Some("accept-edits"));
                assert_eq!(effort, Some(Effort::High));
                assert!(trust && json);
            }
            other => panic!("not the run command: {other:?}"),
        }
        let bare = Cli::try_parse_from(["coolcode", "run"]).expect("parse");
        assert!(matches!(bare.command, Some(Command::Run { prompt, .. }) if prompt.is_empty()));
    }

    #[test]
    fn the_command_line_accepts_both_names() {
        use clap::ValueEnum;
        assert_eq!(
            Effort::from_str("ultimate", true).ok(),
            Some(Effort::Ultimate)
        );
        assert_eq!(
            Effort::from_str("extreme", true).ok(),
            Some(Effort::Ultimate)
        );
    }

    use super::{Cli, Command, Effort, PulseMode, Settings, migrate_settings, settings_path};
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("harness-migrate-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn migration_copies_legacy_config_and_keeps_it() {
        let dir = temp_dir("copy");
        let legacy = dir.join("legacy").join("config.toml");
        let new = dir.join("new").join("config.toml");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, "permission_mode = \"auto\"\n").unwrap();
        assert!(migrate_settings(&new, &legacy).expect("migrate"));
        assert_eq!(
            fs::read_to_string(&new).unwrap(),
            "permission_mode = \"auto\"\n"
        );
        assert!(legacy.exists());
    }

    #[test]
    fn migration_skips_when_new_config_exists() {
        let dir = temp_dir("skip");
        let legacy = dir.join("legacy.toml");
        let new = dir.join("new.toml");
        fs::write(&legacy, "permission_mode = \"auto\"\n").unwrap();
        fs::write(&new, "permission_mode = \"plan\"\n").unwrap();
        assert!(!migrate_settings(&new, &legacy).expect("migrate"));
        assert_eq!(
            fs::read_to_string(&new).unwrap(),
            "permission_mode = \"plan\"\n"
        );
        assert!(
            !migrate_settings(
                &dir.join("absent-new.toml"),
                &dir.join("absent-legacy.toml")
            )
            .unwrap()
        );
    }

    #[test]
    fn old_config_without_new_fields_loads_with_defaults() {
        let settings: Settings = toml::from_str("permission_mode = \"auto\"\n").expect("parse");
        assert_eq!(settings.pulse, PulseMode::Words);
        assert!(!settings.motion_prompt_answered);
        assert_eq!(settings.max_tool_rounds, 40);
        assert!(!settings.stats_enabled && !settings.stats_prompt_answered);
        assert!(settings.background_animation);
        let round_trip: Settings = toml::from_str(
            &toml::to_string(&Settings {
                pulse: PulseMode::Characters,
                ..settings
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(round_trip.pulse, PulseMode::Characters);
    }

    #[test]
    fn tests_never_touch_the_real_config_file() {
        let path = settings_path().expect("settings path");
        assert!(
            path.starts_with(std::env::temp_dir()),
            "tests must use a temporary config, got {}",
            path.display()
        );
    }
}
