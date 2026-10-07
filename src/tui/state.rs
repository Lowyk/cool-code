use crate::agent::{PendingEvent, ToolApproval};
use crate::policy::{MODES, mode_label};
use crate::tui::effort::effort_name;
use crate::{ChainModel, Effort, PulseMode, Settings, provider, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyModifiers};
use std::sync::mpsc::Receiver;
use std::time::Duration;

pub(super) const LEVELS: [Effort; 7] = [
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
    Effort::Super,
    Effort::Ultimate,
];

/// How long newly arrived text keeps glowing; matches the pulse fade in the renderer.
const PULSE_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

/// Live state of the turn currently streaming from the provider.
pub(super) struct StreamingTurn {
    pub(super) text: String,
    pub(super) arrivals: Vec<(usize, std::time::Instant)>,
    pub(super) started: std::time::Instant,
    pub(super) usage: Option<u64>,
    pub(super) tool: Option<(String, std::time::Instant)>,
    pub(super) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl StreamingTurn {
    /// Drops arrival records whose pulse has finished, so redraw cost stays flat on long answers.
    pub(super) fn prune_arrivals(&mut self, now: std::time::Instant) {
        self.arrivals
            .retain(|(_, at)| now.saturating_duration_since(*at) < PULSE_WINDOW);
    }

    pub(super) fn new(cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            text: String::new(),
            arrivals: Vec::new(),
            started: std::time::Instant::now(),
            usage: None,
            tool: None,
            cancel,
        }
    }
}

pub(super) struct App {
    pub(super) settings: Settings,
    pub(super) input: String,
    pub(super) picker: bool,
    pub(super) picker_index: usize,
    /// The workflows checkbox in the effort picker, before it is confirmed.
    pub(super) picker_workflows: bool,
    /// Whether the picker cursor is on the workflows checkbox instead of a column.
    pub(super) picker_focus_workflows: bool,
    pub(super) confirm_ultimate: bool,
    pub(super) privacy_confirmation: Option<PrivacyPrompt>,
    pub(super) pending_privacy_message: Option<provider::ChatMessage>,
    pub(super) trust_prompt: bool,
    pub(super) wizard: Option<crate::tui::setup::SetupWizard>,
    pub(super) workspace_trusted: bool,
    /// Where the per-project choices (trust, CLAUDE.md / AGENTS.md loading) are kept.
    pub(super) projects_path: std::path::PathBuf,
    pub(super) trust_choice: usize,
    pub(super) tool_approval: Option<ToolApproval>,
    pub(super) approval_scroll: u16,
    pub(super) messages: Vec<provider::ChatMessage>,
    pub(super) transcript: Vec<TranscriptEntry>,
    pub(super) pending: Option<Receiver<PendingEvent>>,
    pub(super) streaming: Option<StreamingTurn>,
    /// Background provider fetches (model lists, usage limits) report through this channel.
    #[cfg_attr(test, allow(dead_code))]
    pub(super) tasks: std::sync::mpsc::Sender<crate::tui::settings::sync::TaskResult>,
    pub(super) task_results: std::sync::mpsc::Receiver<crate::tui::settings::sync::TaskResult>,
    pub(super) limits: std::collections::HashMap<String, crate::tui::settings::sync::LimitsEntry>,
    pub(super) models_loading: std::collections::HashSet<String>,
    /// Per-provider description of the saved key's shape (never the key itself).
    pub(super) key_shapes: std::collections::HashMap<String, String>,
    /// The most serious running-low warning for the active provider, shown in the status line.
    pub(super) usage_warning: Option<crate::tui::usage_warnings::UsageWarning>,
    /// Warnings already announced, so each is said only once.
    pub(super) announced_warnings: std::collections::HashSet<String>,
    /// The largest balance seen per provider, to judge a shrinking balance against.
    pub(super) peak_balances: std::collections::HashMap<String, u64>,
    pub(super) stats_view: Option<crate::tui::stats_view::StatsView>,
    #[cfg(test)]
    pub(super) spawned_tasks: usize,
    pub(super) history_scroll: u16,
    pub(super) settings_view: Option<crate::tui::settings::SettingsView>,
    pub(super) provider_index: usize,
    pub(super) provider_form: Option<ProviderDraft>,
    pub(super) chain_form: Option<ChainDraft>,
    pub(super) chain_index: usize,
    pub(super) model_choices: Option<Vec<(usize, String, String)>>,
    pub(super) model_choice_index: usize,
    pub(super) pending_model: Option<String>,
    pub(super) model_picker: Option<crate::tui::pickers::model::ModelPicker>,
    pub(super) session_picker: Option<crate::tui::sessions::SessionPicker>,
    /// Where the prompt cursor belongs after the latest draw (`None`: hidden).
    pub(super) cursor: std::cell::Cell<Option<ratatui::layout::Position>>,
    pub(super) session_id: String,
    pub(super) session_created: i64,
    pub(super) session_dir: std::path::PathBuf,
    pub(super) mode_picker: bool,
    pub(super) mode_index: usize,
    pub(super) effort_flash_until: Option<std::time::Instant>,
    pub(super) notice: String,
    pub(super) running: bool,
    pub(super) launched_at: std::time::Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranscriptKind {
    User,
    Assistant,
    CommandOutput,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TranscriptEntry {
    pub(super) kind: TranscriptKind,
    pub(super) text: String,
}

pub(super) struct PrivacyPrompt {
    pub(super) risk: String,
    pub(super) allow_images: bool,
}

pub(super) struct ProviderDraft {
    pub(super) choosing_preset: bool,
    pub(super) existing_id: Option<String>,
    pub(super) preset: usize,
    pub(super) alias: String,
    /// Shown as a placeholder while `alias` is empty, and used when the field is left blank.
    pub(super) suggested_alias: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) models: Vec<ModelDraft>,
    /// Optional endpoints for custom OpenAI-compatible providers (path or full URL).
    pub(super) models_endpoint: String,
    pub(super) limits_endpoint: String,
    /// The provider's models come from its models endpoint; the form leaves them alone.
    pub(super) managed_models: bool,
    pub(super) focus: usize,
}

pub(super) struct ModelDraft {
    pub(super) id: String,
    pub(super) name: String,
}

pub(super) struct ChainDraft {
    pub(super) original_id: Option<String>,
    pub(super) alias: String,
    pub(super) id: String,
    pub(super) members: Vec<ChainModel>,
    pub(super) activate_on_select: bool,
    pub(super) focus: usize,
    pub(super) member_index: usize,
    pub(super) picking_member: bool,
    pub(super) candidate_index: usize,
}

pub(super) struct ProviderPreset {
    /// Stable identifier; code must match presets by this, never by list position.
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) adapter: &'static str,
    pub(super) base_url: Option<&'static str>,
    pub(super) custom: bool,
    pub(super) models: &'static [(&'static str, &'static str)],
    /// Built-in endpoint paths, relative to `base_url`, for listing models and reading limits.
    pub(super) models_path: Option<&'static str>,
    pub(super) limits_path: Option<&'static str>,
    /// Keys for this provider always start with this; used to catch pasted keys that are mangled.
    pub(super) key_prefix: Option<&'static str>,
}

pub(super) const PROVIDER_PRESETS: [ProviderPreset; 7] = [
    ProviderPreset {
        id: "openai",
        label: "OpenAI (ChatGPT)",
        adapter: "openai",
        base_url: None,
        custom: false,
        models: &[],
        models_path: None,
        limits_path: None,
        key_prefix: None,
    },
    ProviderPreset {
        id: "anthropic",
        label: "Anthropic (Claude)",
        adapter: "anthropic",
        base_url: None,
        custom: false,
        models: &[],
        models_path: None,
        limits_path: None,
        key_prefix: None,
    },
    ProviderPreset {
        id: "google",
        label: "Google (Gemini)",
        adapter: "google",
        base_url: None,
        custom: false,
        models: &[
            ("gemini-flash-latest", ""),
            ("gemini-flash-lite-latest", ""),
            ("gemini-pro-latest", ""),
        ],
        models_path: None,
        limits_path: None,
        key_prefix: None,
    },
    ProviderPreset {
        id: "openrouter",
        label: "OpenRouter",
        adapter: "openai-compatible",
        base_url: Some("https://openrouter.ai/api/v1"),
        custom: false,
        models: &[],
        models_path: None,
        limits_path: None,
        key_prefix: None,
    },
    ProviderPreset {
        id: "multiai",
        label: "MultiAI",
        adapter: "openai-compatible",
        base_url: Some("https://multiai.store/v1"),
        custom: false,
        models: &[],
        models_path: Some("models"),
        limits_path: Some("subscription/limits"),
        key_prefix: Some("ma-live-"),
    },
    ProviderPreset {
        id: "anthropic-custom",
        label: "Custom Anthropic-compatible API",
        adapter: "anthropic-compatible",
        base_url: None,
        custom: true,
        models: &[],
        models_path: None,
        limits_path: None,
        key_prefix: None,
    },
    ProviderPreset {
        id: "openai-custom",
        label: "Custom OpenAI-compatible API",
        adapter: "openai-compatible",
        base_url: None,
        custom: true,
        models: &[],
        models_path: None,
        limits_path: None,
        key_prefix: None,
    },
];

pub(super) fn mode_alias(mode: &str) -> String {
    match mode {
        "accept-edits" => "edits".to_owned(),
        "accept-minimal" => "minimal".to_owned(),
        "accept-everything" => "all".to_owned(),
        other => other.to_owned(),
    }
}

pub(super) fn edit_string(value: &mut String, key: event::KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            value.pop();
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            value.push(character)
        }
        _ => {}
    }
}

impl App {
    pub(super) fn new(settings: Settings) -> Self {
        let mut app = Self::from_settings(settings);
        if app.settings.enforce_workflow_lock() {
            app.notice = format!(
                "Effort set to {}: Super and Ultimate are locked until Dynamic workflows is turned on in Settings → General.",
                effort_name(app.settings.effort)
            );
        }
        if crate::settings_were_migrated() {
            app.notice =
                "Settings moved to ~/.coolcode/config.toml; the old file was kept as a backup."
                    .to_owned();
        }
        app
    }

    fn from_settings(mut settings: Settings) -> Self {
        let (tasks, task_results) = std::sync::mpsc::channel();
        if settings.default_provider_id.is_none() {
            settings.default_provider_id = settings.active_provider_id.clone();
        }
        let picker_index = LEVELS
            .iter()
            .position(|level| *level == settings.effort)
            .unwrap_or(2);
        let provider_index = settings
            .active_provider_id
            .as_deref()
            .and_then(|id| {
                settings
                    .providers
                    .iter()
                    .position(|profile| profile.id == id)
            })
            .unwrap_or(0);
        let mode_index = MODES
            .iter()
            .position(|(_, mode)| *mode == settings.permission_mode)
            .unwrap_or(4);
        let projects_path = crate::projects::default_path();
        let workspace_trusted = std::env::current_dir()
            .ok()
            .is_some_and(|root| crate::projects::is_trusted_at(&projects_path, &root));
        let session_created = chrono::Utc::now().timestamp();
        Self {
            settings,
            input: String::new(),
            picker: false,
            picker_index,
            picker_workflows: false,
            picker_focus_workflows: false,
            confirm_ultimate: false,
            privacy_confirmation: None,
            pending_privacy_message: None,
            trust_prompt: !workspace_trusted,
            wizard: None,
            workspace_trusted,
            projects_path,
            trust_choice: 1,
            tool_approval: None,
            approval_scroll: 0,
            messages: Vec::new(),
            transcript: Vec::new(),
            pending: None,
            streaming: None,
            tasks,
            task_results,
            limits: std::collections::HashMap::new(),
            models_loading: std::collections::HashSet::new(),
            key_shapes: std::collections::HashMap::new(),
            stats_view: None,
            #[cfg(test)]
            spawned_tasks: 0,
            history_scroll: 0,
            settings_view: None,
            provider_index,
            provider_form: None,
            chain_form: None,
            chain_index: 0,
            model_choices: None,
            model_choice_index: 0,
            pending_model: None,
            model_picker: None,
            session_picker: None,
            usage_warning: None,
            announced_warnings: std::collections::HashSet::new(),
            peak_balances: std::collections::HashMap::new(),
            cursor: std::cell::Cell::new(None),
            session_id: crate::session::new_id(session_created),
            session_created,
            session_dir: crate::tui::sessions::default_session_dir(),
            mode_picker: false,
            mode_index,
            effort_flash_until: None,
            notice: if workspace_trusted {
                "Trusted workspace · type a task or use /help for commands.".to_owned()
            } else {
                "Workspace access is paused until you trust this folder or decline.".to_owned()
            },
            running: true,
            launched_at: std::time::Instant::now(),
        }
    }

    pub(super) fn answer_stats_prompt(&mut self, enable: bool) -> Result<()> {
        self.settings.stats_enabled = enable;
        self.settings.stats_prompt_answered = true;
        write_settings(&self.settings)?;
        self.notice = if enable {
            "Usage stats are recorded on this computer. See them with /stats.".to_owned()
        } else {
            "Usage stats stay off. Turn them on any time in Settings.".to_owned()
        };
        Ok(())
    }

    pub(super) fn answer_motion_prompt(&mut self, reduce_motion: bool) -> Result<()> {
        if reduce_motion {
            self.settings.pulse = PulseMode::Off;
            self.settings.background_animation = false;
            self.settings.backdrop_in_chat = false;
        }
        self.settings.motion_prompt_answered = true;
        write_settings(&self.settings)
    }

    pub(super) fn finish_command(&mut self, output: impl Into<String>) {
        self.transcript.push(TranscriptEntry {
            kind: TranscriptKind::CommandOutput,
            text: output.into(),
        });
        self.history_scroll = 0;
    }

    pub(super) fn choose_effort(&mut self) -> Result<()> {
        let levels = self.picker_levels();
        let selected = levels[self.picker_index.min(levels.len() - 1)];
        if self.workflow_tier_is_locked(selected) {
            return Ok(());
        }
        if matches!(selected, Effort::Ultimate) && !self.settings.ultimate_acknowledged {
            self.confirm_ultimate = true;
            return Ok(());
        }
        // The checkbox only belongs to the plain levels; the tiers are workflows by definition.
        self.settings.workflows = self.picker_workflows
            && self.settings.dynamic_workflows
            && matches!(selected, Effort::Low | Effort::Medium | Effort::High);
        self.apply_effort(selected)
    }

    /// True (after telling the user why) when `effort` is a workflow tier that is still locked.
    pub(super) fn workflow_tier_is_locked(&mut self, effort: Effort) -> bool {
        let locked = effort.is_workflow_tier() && !self.settings.dynamic_workflows;
        if locked {
            self.notice = format!(
                "{} is locked. Turn on Dynamic workflows in Settings → General to use it.",
                effort_name(effort)
            );
        }
        locked
    }

    pub(super) fn apply_effort(&mut self, effort: Effort) -> Result<()> {
        if self.workflow_tier_is_locked(effort) {
            self.picker = false;
            return Ok(());
        }
        self.settings.effort = effort;
        write_settings(&self.settings)?;
        self.picker = false;
        self.confirm_ultimate = false;
        self.effort_flash_until = Some(
            std::time::Instant::now() + Duration::from_secs_f32(crate::tui::effort::FLASH_SECONDS),
        );
        self.notice = format!("Effort set to {}.", effort_name(effort));
        Ok(())
    }

    pub(super) fn apply_mode(&mut self, mode: &str) -> Result<()> {
        self.settings.permission_mode = mode.to_owned();
        self.mode_index = MODES
            .iter()
            .position(|(_, value)| *value == mode)
            .unwrap_or(4);
        write_settings(&self.settings)?;
        self.mode_picker = false;
        self.notice = format!("Mode set to {}.", mode_label(mode));
        Ok(())
    }
}
