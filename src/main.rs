use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

mod agent;
mod policy;
mod provider;
mod secrets;
mod stream;
mod tools;
mod tui;

#[derive(Debug, Parser)]
#[command(name = "harness", version, about = "A coding-focused AI harness")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
    /// Show or set the reasoning/workflow effort level.
    Effort {
        /// Effort to select: low, medium, high, max, xhigh, super, or extreme.
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
    Extreme,
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
            Self::Extreme => {
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
    extreme_acknowledged: bool,
    background_animation: bool,
    pulse: PulseMode,
    motion_prompt_answered: bool,
    max_tool_rounds: usize,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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
            extreme_acknowledged: false,
            background_animation: true,
            pulse: PulseMode::Words,
            motion_prompt_answered: false,
            max_tool_rounds: 40,
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
    Ok(std::env::temp_dir()
        .join(format!("harness-test-{}", std::process::id()))
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
    let Some(command) = Cli::parse().command else {
        return tui::run();
    };
    match command {
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
                if matches!(level, Effort::Extreme) {
                    eprintln!(
                        "Warning: Extreme may use substantially more tokens and incur higher cost."
                    );
                    eprintln!(
                        "Dynamic workflows are not implemented yet; this setting is saved for the roadmap."
                    );
                    eprint!("Set Extreme effort anyway? [y/N] ");
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
                    Effort::Extreme,
                ] {
                    println!(
                        "  {:<8} {}",
                        format!("{:?}", effort).to_lowercase(),
                        effort.description()
                    );
                }
                println!("Use `harness effort <level>` to select one.");
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
    use super::{PulseMode, Settings, migrate_settings, settings_path};
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
