use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::tui::widgets::tree::{ModelTarget, draw_tree};
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
            let rows = view.tree.rows(&app.settings, false);
            draw_tree(
                frame,
                area,
                &rows,
                view.tree.selected,
                view.focus == Focus::Content,
            );
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
        let rows = view.tree.rows(&self.settings, false);
        let target = view.tree.current(&rows).and_then(|row| row.target.clone());
        if view.confirm_delete {
            view.confirm_delete = false;
            if key.code == KeyCode::Char('y')
                && let Some(ModelTarget {
                    provider,
                    index: Some(model),
                    ..
                }) = target
            {
                self.remove_model(provider, model)?;
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
            KeyCode::Up => view.tree.move_by(&rows, -1),
            KeyCode::Down => view.tree.move_by(&rows, 1),
            KeyCode::Right => view.tree.expand(&rows),
            KeyCode::Left => view.tree.collapse(&rows),
            KeyCode::Enter | KeyCode::Char(' ') => view.tree.toggle(&rows),
            KeyCode::Char('n') => view.model_edit = Some(ModelEdit::ChooseProvider { choice: 0 }),
            KeyCode::Char('r') => match target {
                Some(ModelTarget {
                    provider,
                    index: Some(model),
                    ..
                }) => {
                    view.model_edit = Some(ModelEdit::Rename {
                        provider,
                        model,
                        text: self.settings.providers[provider].models[model].name.clone(),
                    });
                }
                _ => self.notice = "Select a model to rename it.".to_owned(),
            },
            KeyCode::Char('x') => match target {
                Some(ModelTarget { index: Some(_), .. }) => view.confirm_delete = true,
                _ => self.notice = "Select a model to remove it.".to_owned(),
            },
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
        for chain in &mut self.settings.model_chains {
            chain.members.retain(|member| {
                !(member.provider_id == provider_id
                    && member.model_id.eq_ignore_ascii_case(&removed.id))
            });
        }
        self.settings
            .model_chains
            .retain(|chain| !chain.members.is_empty());
        if let Some(active) = self.settings.active_chain_id.as_deref()
            && !self
                .settings
                .model_chains
                .iter()
                .any(|chain| chain.id == active)
        {
            self.settings.active_chain_id = None;
        }
        self.notice = format!("Removed {}.", removed.id);
        write_settings(&self.settings)
    }
}

#[cfg(test)]
mod tests {
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::tui::widgets::tree::RowKind;
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
            ..Default::default()
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

    /// Unfolds the provider (if needed) and puts the cursor on one of its models.
    fn select_model_row(app: &mut App, provider_id: &str, model_id: &str) {
        let view = app.settings_view.as_mut().expect("settings open");
        let rows = view.tree.rows(&app.settings, false);
        let header = rows
            .iter()
            .position(|row| row.kind == RowKind::Provider && row.label == provider_id)
            .expect("provider row");
        view.tree.selected = header;
        view.tree.expand(&rows);
        let rows = view.tree.rows(&app.settings, false);
        view.tree.selected = rows
            .iter()
            .position(|row| row.target.as_ref().is_some_and(|t| t.id == model_id))
            .expect("model row");
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn models_section_is_a_tree_with_only_the_active_provider_open() {
        let app = app();
        let view = app.settings_view.as_ref().expect("view");
        let rows = view.tree.rows(&app.settings, false);
        let shown = rows
            .iter()
            .map(|row| (row.kind, row.label.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(shown[0], (RowKind::Provider, "google"));
        assert_eq!(shown[1], (RowKind::Provider, "groq"));
        assert!(
            rows.iter()
                .any(|row| row.label == "Qwen3.8-27B (qwen/qwen3.8-27b)"),
            "{:?}",
            rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>()
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.label.contains("gemini-pro-latest"))
        );
    }

    #[test]
    fn enter_unfolds_a_provider_and_the_arrows_move_between_rows() {
        let mut app = app();
        {
            let view = app.settings_view.as_mut().expect("view");
            view.tree.selected = 0;
        }
        press(&mut app, KeyCode::Enter);
        let view = app.settings_view.as_ref().expect("view");
        let rows = view.tree.rows(&app.settings, false);
        assert!(
            rows.iter()
                .any(|row| row.label.contains("gemini-pro-latest"))
        );
        press(&mut app, KeyCode::Down);
        let view = app.settings_view.as_ref().expect("view");
        assert_eq!(view.tree.selected, 1);
    }

    #[test]
    fn rename_and_remove_need_a_model_under_the_cursor() {
        let mut app = app();
        app.settings_view.as_mut().expect("view").tree.selected = 0;
        press(&mut app, KeyCode::Char('r'));
        assert!(app.notice.contains("Select a model"), "{}", app.notice);
        assert!(
            app.settings_view
                .as_ref()
                .is_some_and(|view| view.model_edit.is_none())
        );
        press(&mut app, KeyCode::Char('x'));
        assert!(
            app.settings_view
                .as_ref()
                .is_some_and(|view| !view.confirm_delete)
        );
    }

    #[test]
    fn models_rename_updates_display_name() {
        let mut app = app();
        select_model_row(&mut app, "groq", "qwen/qwen3.8-27b");
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
        select_model_row(&mut app, "groq", "qwen/qwen3.8-27b");
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
        select_model_row(&mut app, "google", "gemini-pro-latest");
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.settings.providers[0].models.len(), 1);
        assert!(app.notice.contains("only model"), "{}", app.notice);
    }

    #[test]
    fn esc_cancels_a_rename_without_leaving_the_section() {
        let mut app = app();
        select_model_row(&mut app, "google", "gemini-pro-latest");
        press(&mut app, KeyCode::Char('r'));
        type_text(&mut app, "zzz");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.settings.providers[0].models[0].name, "");
        assert_eq!(
            app.settings_view.as_ref().map(|v| v.section),
            Some(Section::Models)
        );
    }

    #[test]
    fn removing_a_model_prunes_it_from_chains() {
        let mut settings = settings();
        settings.model_chains = vec![
            crate::ModelChain {
                id: "mixed".to_owned(),
                alias: String::new(),
                members: vec![
                    crate::ChainModel {
                        provider_id: "groq".to_owned(),
                        model_id: "qwen/qwen3.8-27b".to_owned(),
                    },
                    crate::ChainModel {
                        provider_id: "google".to_owned(),
                        model_id: "gemini-pro-latest".to_owned(),
                    },
                ],
                activate_on_select: false,
            },
            crate::ModelChain {
                id: "solo".to_owned(),
                alias: String::new(),
                members: vec![crate::ChainModel {
                    provider_id: "groq".to_owned(),
                    model_id: "qwen/qwen3.8-27b".to_owned(),
                }],
                activate_on_select: false,
            },
        ];
        settings.active_chain_id = Some("solo".to_owned());
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.open_settings(Section::Models);
        press(&mut app, KeyCode::Right);
        select_model_row(&mut app, "groq", "qwen/qwen3.8-27b");
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.settings.model_chains.len(), 1);
        assert_eq!(app.settings.model_chains[0].members.len(), 1);
        assert_eq!(
            app.settings.model_chains[0].members[0].provider_id,
            "google"
        );
        assert_eq!(app.settings.active_chain_id, None);
    }
}
