use crate::policy::MODES;
use crate::tui::effort::{effort_name, effort_style};
use crate::tui::models::selected_model_name;
use crate::tui::pickers::model::ModelPicker;
use crate::tui::render::mode_span;
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::{App, LEVELS};
use crate::{PulseMode, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub(super) const ROWS: usize = 7;

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
        vec![if app.settings.background_animation {
            Span::styled("on", Style::default().fg(Color::Rgb(110, 220, 130)))
        } else {
            Span::styled("off", Style::default().fg(Color::Gray))
        }],
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
    ];
    let labels = [
        "Model",
        "Effort",
        "Mode",
        "Workspace trust",
        "Background",
        "Pulse",
        "Usage stats",
    ];
    let focused = view.focus == Focus::Content;
    let mut lines = Vec::new();
    for (index, (label, value)) in labels.iter().zip(values).enumerate() {
        let selected = focused && index == view.row;
        let mut spans = vec![
            Span::styled(
                if selected { "▸ " } else { "  " },
                Style::default().fg(Color::Rgb(98, 213, 244)),
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
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "API keys are kept in the OS credential store, never in the config file.",
        Style::default().fg(Color::DarkGray),
    )));
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
                    self.picker_index = LEVELS
                        .iter()
                        .position(|level| *level == self.settings.effort)
                        .unwrap_or(0);
                    self.picker = true;
                }
                2 => {
                    self.mode_index = MODES
                        .iter()
                        .position(|(_, mode)| *mode == self.settings.permission_mode)
                        .unwrap_or(0);
                    self.mode_picker = true;
                }
                3 => self.set_workspace_trusted(!self.workspace_trusted)?,
                4 => {
                    self.settings.background_animation = !self.settings.background_animation;
                    write_settings(&self.settings)?;
                }
                6 => {
                    self.settings.stats_enabled = !self.settings.stats_enabled;
                    self.settings.stats_prompt_answered = true;
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
