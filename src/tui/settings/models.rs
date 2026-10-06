use crate::tui::models::model_name;
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::tui::widgets::list::{ListItem, ListState, draw_list};
use crate::{ModelProfile, Settings, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum ModelEdit {
    Rename {
        provider: usize,
        model: usize,
        text: String,
    },
    ChooseProvider {
        choice: usize,
    },
    NewId {
        provider: usize,
        text: String,
    },
    NewName {
        provider: usize,
        id: String,
        text: String,
    },
}

pub(super) fn model_rows(settings: &Settings) -> Vec<(ListItem, (usize, usize))> {
    let mut rows = Vec::new();
    for (provider_index, profile) in settings.providers.iter().enumerate() {
        if profile.draft {
            continue;
        }
        for (model_index, model) in profile.models.iter().enumerate() {
            rows.push((
                ListItem {
                    label: if model.name.is_empty() {
                        model_name(&model.id)
                    } else {
                        model.name.clone()
                    },
                    detail: format!("{} · {}", model.id, profile.name),
                    selectable: true,
                    ..ListItem::default()
                },
                (provider_index, model_index),
            ));
        }
    }
    rows
}

fn provider_choices(settings: &Settings) -> Vec<usize> {
    settings
        .providers
        .iter()
        .enumerate()
        .filter(|(_, profile)| !profile.draft)
        .map(|(index, _)| index)
        .collect()
}

pub(super) fn draw_models(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let accent = Color::Rgb(98, 213, 244);
    let prompt = |label: &str, text: &str| {
        Line::from(vec![
            Span::styled(format!("{label}: "), Style::default().fg(Color::Gray)),
            Span::styled(text.to_owned(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(accent)),
        ])
    };
    let lines = match &view.model_edit {
        Some(ModelEdit::Rename { text, .. }) => vec![prompt("Display name", text)],
        Some(ModelEdit::NewId { provider, text }) => vec![
            Line::from(format!(
                "Add a model to {}",
                app.settings.providers[*provider].name
            )),
            Line::from(""),
            prompt("Model ID", text),
        ],
        Some(ModelEdit::NewName { id, text, .. }) => vec![
            Line::from(format!("Model ID: {id}")),
            Line::from(""),
            prompt("Display name (optional)", text),
        ],
        Some(ModelEdit::ChooseProvider { choice }) => {
            let mut lines = vec![Line::from("Add a model to which provider?"), Line::from("")];
            for (position, index) in provider_choices(&app.settings).into_iter().enumerate() {
                let selected = position == *choice;
                lines.push(Line::from(vec![
                    Span::styled(
                        if selected { "▸ " } else { "  " },
                        Style::default().fg(accent),
                    ),
                    Span::styled(
                        app.settings.providers[index].name.clone(),
                        if selected {
                            Style::default().fg(accent).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Gray)
                        },
                    ),
                ]));
            }
            lines
        }
        None => {
            let rows = model_rows(&app.settings);
            let items = rows.into_iter().map(|(item, _)| item).collect::<Vec<_>>();
            let state = ListState {
                selected: view.row,
                filter: String::new(),
            };
            draw_list(frame, area, &items, &state, view.focus == Focus::Content);
            return;
        }
    };
    frame.render_widget(Paragraph::new(lines), area);
}

impl App {
    pub(super) fn handle_models_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        let rows = model_rows(&self.settings);
        let row = view.row.min(rows.len().saturating_sub(1));
        if view.confirm_delete {
            view.confirm_delete = false;
            if key.code == KeyCode::Char('y')
                && let Some((_, (provider, model))) = rows.get(row)
            {
                self.remove_model(*provider, *model)?;
            } else {
                self.notice = "Removal cancelled.".to_owned();
            }
            return Ok(());
        }
        if let Some(edit) = view.model_edit.take() {
            if key.code == KeyCode::Esc {
                return Ok(());
            }
            let next = self.advance_model_edit(edit, key)?;
            if let Some(view) = self.settings_view.as_mut() {
                view.model_edit = next;
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Up => view.row = row.saturating_sub(1),
            KeyCode::Down => view.row = (row + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Char('n') => view.model_edit = Some(ModelEdit::ChooseProvider { choice: 0 }),
            KeyCode::Char('r') => {
                if let Some((_, (provider, model))) = rows.get(row) {
                    view.model_edit = Some(ModelEdit::Rename {
                        provider: *provider,
                        model: *model,
                        text: self.settings.providers[*provider].models[*model]
                            .name
                            .clone(),
                    });
                }
            }
            KeyCode::Char('x') if !rows.is_empty() => view.confirm_delete = true,
            _ => {}
        }
        Ok(())
    }

    fn advance_model_edit(
        &mut self,
        edit: ModelEdit,
        key: event::KeyEvent,
    ) -> Result<Option<ModelEdit>> {
        let edit_text = |text: &mut String| match key.code {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            _ => {}
        };
        Ok(match edit {
            ModelEdit::ChooseProvider { choice } => {
                let choices = provider_choices(&self.settings);
                match key.code {
                    KeyCode::Up => Some(ModelEdit::ChooseProvider {
                        choice: choice.saturating_sub(1),
                    }),
                    KeyCode::Down => Some(ModelEdit::ChooseProvider {
                        choice: (choice + 1).min(choices.len().saturating_sub(1)),
                    }),
                    KeyCode::Enter => choices.get(choice).map(|provider| ModelEdit::NewId {
                        provider: *provider,
                        text: String::new(),
                    }),
                    _ => Some(ModelEdit::ChooseProvider { choice }),
                }
            }
            ModelEdit::NewId { provider, mut text } => {
                if key.code != KeyCode::Enter {
                    edit_text(&mut text);
                    return Ok(Some(ModelEdit::NewId { provider, text }));
                }
                let id = text.trim().to_owned();
                let exists = self.settings.providers[provider]
                    .models
                    .iter()
                    .any(|model| model.id.eq_ignore_ascii_case(&id));
                if id.is_empty() || exists {
                    self.notice = if exists {
                        format!("{id} is already listed for this provider.")
                    } else {
                        "Enter a model ID.".to_owned()
                    };
                    Some(ModelEdit::NewId { provider, text })
                } else {
                    Some(ModelEdit::NewName {
                        provider,
                        id,
                        text: String::new(),
                    })
                }
            }
            ModelEdit::NewName {
                provider,
                id,
                mut text,
            } => {
                if key.code != KeyCode::Enter {
                    edit_text(&mut text);
                    return Ok(Some(ModelEdit::NewName { provider, id, text }));
                }
                let profile = &mut self.settings.providers[provider];
                profile.models.push(ModelProfile {
                    id: id.clone(),
                    name: text.trim().to_owned(),
                });
                if profile.model.is_empty() {
                    profile.model = id.clone();
                }
                self.notice = format!("Added {id} to {}.", profile.name);
                write_settings(&self.settings)?;
                None
            }
            ModelEdit::Rename {
                provider,
                model,
                mut text,
            } => {
                if key.code != KeyCode::Enter {
                    edit_text(&mut text);
                    return Ok(Some(ModelEdit::Rename {
                        provider,
                        model,
                        text,
                    }));
                }
                self.settings.providers[provider].models[model].name = text.trim().to_owned();
                self.notice = "Model renamed.".to_owned();
                write_settings(&self.settings)?;
                None
            }
        })
    }

    fn remove_model(&mut self, provider: usize, model: usize) -> Result<()> {
        let profile = &mut self.settings.providers[provider];
        if profile.models.len() <= 1 {
            self.notice = format!(
                "{} needs at least one model; this is its only model. Delete the provider instead.",
                profile.name
            );
            return Ok(());
        }
        let removed = profile.models.remove(model);
        if profile.model == removed.id {
            profile.model = profile.models[0].id.clone();
        }
        let replacement = profile.model.clone();
        let provider_id = profile.id.clone();
        if self.settings.active_provider_id.as_deref() == Some(provider_id.as_str())
            && self.settings.model.as_deref() == Some(removed.id.as_str())
        {
            self.settings.model = Some(replacement);
        }
        self.notice = format!("Removed {}.", removed.id);
        write_settings(&self.settings)
    }
}

#[cfg(test)]
mod tests {
    use super::model_rows;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{ModelProfile, ProviderProfile, Settings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn profile(id: &str, models: &[&str], draft: bool) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: id.to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: models[0].to_owned(),
            models: models
                .iter()
                .map(|model| ModelProfile {
                    id: (*model).to_owned(),
                    name: String::new(),
                })
                .collect(),
            draft,
            auto_switch: true,
            base_url: None,
        }
    }

    fn settings() -> Settings {
        let mut settings = Settings::default();
        settings.providers = vec![
            profile("google", &["gemini-pro-latest"], false),
            profile("groq", &["qwen/qwen3.8-27b", "openai/gpt-oss-120b"], false),
            profile("wip", &["draft-model"], true),
        ];
        settings.active_provider_id = Some("groq".to_owned());
        settings.model = Some("qwen/qwen3.8-27b".to_owned());
        settings
    }

    fn app() -> App {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.open_settings(Section::Models);
        press(&mut app, KeyCode::Right);
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn models_section_lists_models_across_providers() {
        let rows = model_rows(&settings());
        let targets = rows.iter().map(|(_, target)| *target).collect::<Vec<_>>();
        assert_eq!(targets, vec![(0, 0), (1, 0), (1, 1)]);
        assert!(rows[2].0.detail.contains("openai/gpt-oss-120b"));
        assert!(rows[2].0.detail.contains("groq"));
    }

    #[test]
    fn models_rename_updates_display_name() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('r'));
        type_text(&mut app, "Qwen Fast");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.providers[1].models[0].name, "Qwen Fast");
    }

    #[test]
    fn models_add_appends_to_chosen_provider() {
        let mut app = app();
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "llama-4");
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "Llama 4");
        press(&mut app, KeyCode::Enter);
        let groq = &app.settings.providers[1];
        assert_eq!(groq.models.len(), 3);
        assert_eq!(groq.models[2].id, "llama-4");
        assert_eq!(groq.models[2].name, "Llama 4");
    }

    #[test]
    fn removing_active_model_moves_provider_to_next_model() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        let groq = &app.settings.providers[1];
        assert_eq!(groq.models.len(), 1);
        assert_eq!(groq.model, "openai/gpt-oss-120b");
        assert_eq!(app.settings.model.as_deref(), Some("openai/gpt-oss-120b"));
    }

    #[test]
    fn removing_only_model_is_refused() {
        let mut app = app();
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.settings.providers[0].models.len(), 1);
        assert!(app.notice.contains("only model"), "{}", app.notice);
    }

    #[test]
    fn esc_cancels_a_rename_without_leaving_the_section() {
        let mut app = app();
        press(&mut app, KeyCode::Char('r'));
        type_text(&mut app, "zzz");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.settings.providers[0].models[0].name, "");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.section),
            Some(Section::Models)
        );
    }
}
