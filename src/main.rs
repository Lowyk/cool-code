use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

mod provider;
mod secrets;
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
    effort: Effort,
    permission_mode: String,
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
            effort: Effort::High,
            permission_mode: "plan".to_owned(),
        }
    }
}

fn settings_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("could not locate the user config directory")?;
    Ok(config_dir.join("harness").join("config.toml"))
}

fn read_settings() -> Result<Settings> {
    let path = settings_path()?;
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
