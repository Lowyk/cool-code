use crate::tui::effort::{effort_name, effort_style};
use crate::tui::models::selected_model_name;
use crate::tui::mouse::record_wrapped;
use crate::tui::pickers::model::ModelPicker;
use crate::tui::render::mode_span;
use crate::tui::settings::reset::ResetStage;
use crate::tui::settings::workflow_size::WorkflowChooser;
use crate::tui::settings::{Focus, SettingsView, content_row};
use crate::tui::state::App;
use crate::{PulseMode, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub(super) const ROWS: usize = 17;

fn switch(on: bool) -> Span<'static> {
    if on {
        Span::styled("on", Style::default().fg(Color::Rgb(110, 220, 130)))
    } else {
        Span::styled("off", Style::default().fg(Color::Gray))
    }
}

/// The Dynamic workflows row: "off", or the size, its limit and how many run at once.
fn workflow_size_value(settings: &crate::Settings) -> Span<'static> {
    if !settings.workflows_unlocked() {
        return switch(false);
    }
    Span::styled(
        format!(
            "{} · up to {} · {} at once",
            settings.workflow_size.label(),
            settings.workflow_limit(),
            settings.subagents_at_once()
        ),
        Style::default().fg(Color::Rgb(255, 197, 92)),
    )
}

pub(super) fn draw_general(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let model = app
        .settings
        .model
        .as_deref()
        .map(|id| selected_model_name(&app.settings, id))
        .unwrap_or_else(|| "not set".to_owned());
    let provider = app
        .settings
        .active_provider_id
        .as_deref()
        .and_then(|id| app.settings.providers.iter().find(|p| p.id == id))
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let trust = if app.workspace_trusted {
        Span::styled("trusted", Style::default().fg(Color::Rgb(110, 220, 130)))
    } else {
        Span::styled("not trusted", Style::default().fg(Color::Gray))
    };
    let values: [Vec<Span<'static>>; ROWS] = [
        vec![
            Span::styled(model, Style::default().fg(Color::White)),
            Span::styled(
                if provider.is_empty() {
                    String::new()
                } else {
                    format!(" · {provider}")
                },
                Style::default().fg(Color::DarkGray),
            ),
        ],
        vec![Span::styled(
            effort_name(app.settings.effort),
            effort_style(app.settings.effort, true),
        )],
        vec![mode_span(&app.settings.permission_mode, true)],
        vec![trust],
        vec![Span::styled(
            match app.settings.pulse {
                PulseMode::Off => "off",
                PulseMode::Words => "words",
                PulseMode::Characters => "characters",
            },
            Style::default().fg(Color::White),
        )],
        vec![if app.settings.stats_enabled {
            Span::styled("on", Style::default().fg(Color::Rgb(110, 220, 130)))
        } else {
            Span::styled("off", Style::default().fg(Color::Gray))
        }],
        vec![if app.settings.sessions_enabled {
            Span::styled("on", Style::default().fg(Color::Rgb(110, 220, 130)))
        } else {
            Span::styled("off", Style::default().fg(Color::Gray))
        }],
        vec![switch(app.settings.default_load_claude_md)],
        vec![switch(app.settings.default_load_agents_md)],
        vec![switch(app.settings.load_global_claude_md)],
        vec![switch(app.settings.load_claude_skills)],
        vec![workflow_size_value(&app.settings)],
        vec![switch(app.settings.usage_warnings)],
        vec![switch(app.settings.auto_compact)],
        vec![match &app.settings.image_generation {
            Some(config) if crate::imagegen::available(&app.settings) => Span::styled(
                config.model.clone(),
                Style::default().fg(Color::Rgb(110, 220, 130)),
            ),
            Some(_) => Span::styled("key missing", Style::default().fg(Color::Rgb(255, 197, 92))),
            None => Span::styled("off", Style::default().fg(Color::Gray)),
        }],
        vec![switch(app.settings.mouse)],
        vec![Span::styled("…", Style::default().fg(Color::DarkGray))],
    ];
    let labels = [
        "Model",
        "Effort",
        "Mode",
        "Workspace trust",
        "Pulse",
        "Usage stats",
        "Save sessions",
        "Load CLAUDE.md",
        "Load AGENTS.md",
        "Global CLAUDE.md",
        "Load Claude skills",
        "Dynamic workflows",
        "Usage warnings",
        "Auto-compact",
        "Image generation",
        "Mouse",
        "Reset",
    ];
    let focused = view.focus == Focus::Content;
    let mut lines = Vec::new();
    let clicks = (0..ROWS)
        .map(|index| (index, content_row(view, index)))
        .collect::<Vec<_>>();
    for (index, (label, value)) in labels.iter().zip(values).enumerate() {
        let selected = focused && index == view.row;
        let mut spans = vec![
            Span::styled(
                if selected { "▸ " } else { "  " },
                Style::default().fg(crate::tui::theme::accent()),
            ),
            Span::styled(
                format!("{label:<17}"),
                if selected {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ];
        spans.extend(value);
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "API keys are kept in the OS credential store, never in the config file.",
        Style::default().fg(Color::DarkGray),
    )));
    record_wrapped(&app.hits, area, &lines, &clicks);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

impl App {
    pub(super) fn handle_general_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Up => view.row = view.row.saturating_sub(1),
            KeyCode::Down => view.row = (view.row + 1).min(ROWS - 1),
            KeyCode::Enter => match view.row {
                0 => self.model_picker = Some(ModelPicker::new(&self.settings)),
                1 => {
                    self.open_effort_picker();
                }
                2 => {
                    self.mode_index = crate::policy::mode_index(&self.settings.permission_mode);
                    self.mode_picker = true;
                }
                3 => self.set_workspace_trusted(!self.workspace_trusted)?,
                5 => {
                    self.settings.stats_enabled = !self.settings.stats_enabled;
                    self.settings.stats_prompt_answered = true;
                    write_settings(&self.settings)?;
                }
                7 => {
                    self.settings.default_load_claude_md = !self.settings.default_load_claude_md;
                    self.settings.instructions_prompt_answered = true;
                    write_settings(&self.settings)?;
                }
                8 => {
                    self.settings.default_load_agents_md = !self.settings.default_load_agents_md;
                    self.settings.instructions_prompt_answered = true;
                    write_settings(&self.settings)?;
                }
                9 => {
                    self.settings.load_global_claude_md = !self.settings.load_global_claude_md;
                    self.settings.instructions_prompt_answered = true;
                    write_settings(&self.settings)?;
                }
                10 => {
                    self.settings.load_claude_skills = !self.settings.load_claude_skills;
                    self.notice = if self.settings.load_claude_skills {
                        "Skills in ~/.claude/skills are loaded too."
                    } else {
                        "Skills in ~/.claude/skills are no longer loaded."
                    }
                    .to_owned();
                    write_settings(&self.settings)?;
                }
                11 => {
                    view.workflow_chooser = Some(WorkflowChooser::open(&self.settings));
                }
                12 => {
                    self.settings.usage_warnings = !self.settings.usage_warnings;
                    if !self.settings.usage_warnings {
                        self.usage_warning = None;
                    }
                    write_settings(&self.settings)?;
                }
                13 => {
                    self.settings.auto_compact = !self.settings.auto_compact;
                    self.notice = if self.settings.auto_compact {
                        "Long conversations are condensed automatically before they fill the context window."
                    } else {
                        "Auto-compact is off; use /compact to condense the conversation yourself."
                    }
                    .to_owned();
                    write_settings(&self.settings)?;
                }
                14 => self.open_image_setup(),
                15 => {
                    self.settings.mouse = !self.settings.mouse;
                    self.notice = if self.settings.mouse {
                        "Mouse on: click buttons and rows, scroll with the wheel. Hold Shift (Option on macOS) to select text."
                    } else {
                        "Mouse off: the terminal selects text as usual. Shift is no longer needed."
                    }
                    .to_owned();
                    write_settings(&self.settings)?;
                }
                16 => view.reset = Some(ResetStage::Menu { row: 0 }),
                6 => {
                    self.settings.sessions_enabled = !self.settings.sessions_enabled;
                    self.settings.sessions_prompt_answered = true;
                    write_settings(&self.settings)?;
                }
                _ => {
                    self.settings.pulse = match self.settings.pulse {
                        PulseMode::Words => PulseMode::Characters,
                        PulseMode::Characters => PulseMode::Off,
                        PulseMode::Off => PulseMode::Words,
                    };
                    write_settings(&self.settings)?;
                }
            },
            _ => {}
        }
        Ok(())
    }
}
