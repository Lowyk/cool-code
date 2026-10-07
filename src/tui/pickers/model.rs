use crate::Settings;
use crate::endpoints::model_tags;
use crate::tui::models::model_name;
use crate::tui::render::centered_rect;
use crate::tui::state::App;
use crate::tui::widgets::list::{ListItem, ListState, draw_list};
use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

pub(in crate::tui) struct ModelPicker {
    pub(in crate::tui) items: Vec<ListItem>,
    pub(in crate::tui) targets: Vec<Option<(usize, String)>>,
    pub(in crate::tui) state: ListState,
}

impl ModelPicker {
    pub(in crate::tui) fn new(settings: &Settings) -> ModelPicker {
        let mut items = Vec::new();
        let mut targets = Vec::new();
        let mut active = None;
        for (index, profile) in settings.providers.iter().enumerate() {
            let models: Vec<(String, String)> = if profile.models.is_empty() {
                (!profile.model.is_empty())
                    .then(|| (profile.model.clone(), String::new()))
                    .into_iter()
                    .collect()
            } else {
                profile
                    .models
                    .iter()
                    .map(|model| (model.id.clone(), model.name.clone()))
                    .collect()
            };
            for (id, name) in models {
                let marked = settings.active_provider_id.as_deref() == Some(profile.id.as_str())
                    && settings.model.as_deref() == Some(id.as_str());
                if marked {
                    active = Some(items.len());
                }
                items.push(ListItem {
                    label: if name.is_empty() {
                        model_name(&id)
                    } else {
                        name
                    },
                    detail: format!("{id}{}", model_tags(profile.model_info.get(&id))),
                    group: Some(profile.name.clone()),
                    dimmed: profile.draft,
                    selectable: !profile.draft,
                    marked,
                });
                targets.push((!profile.draft).then_some((index, id)));
            }
        }
        let mut state = ListState::default();
        if let Some(active) = active {
            state.select_item(&items, active);
        }
        ModelPicker {
            items,
            targets,
            state,
        }
    }

    pub(in crate::tui) fn current_target(&self) -> Option<&(usize, String)> {
        self.state
            .current(&self.items)
            .and_then(|index| self.targets[index].as_ref())
    }
}

impl App {
    pub(in crate::tui) fn handle_model_picker_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(picker) = self.model_picker.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Up => picker.state.move_by(&picker.items, -1),
            KeyCode::Down => picker.state.move_by(&picker.items, 1),
            KeyCode::Backspace => picker.state.pop_filter(&picker.items),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.state.push_filter(&picker.items, c)
            }
            KeyCode::Enter => {
                if let Some((provider_index, model_id)) = picker.current_target().cloned() {
                    self.model_picker = None;
                    self.activate_model(provider_index, &model_id)?;
                }
            }
            KeyCode::Esc => self.model_picker = None,
            _ => {}
        }
        Ok(())
    }
}

pub(in crate::tui) fn draw_model_picker(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    picker: &ModelPicker,
) {
    let popup = centered_rect(64, 70, area);
    frame.render_widget(Clear, popup);
    let accent = Color::Rgb(120, 220, 245);
    let block = Block::default()
        .title(" Model ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(25, 32, 38)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 4 {
        return;
    }
    let filter = Line::from(vec![
        Span::styled("> ", Style::default().fg(accent)),
        Span::raw(picker.state.filter.clone()),
        Span::styled("_", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(
        Paragraph::new(filter),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 3);
    draw_list(frame, list_area, &picker.items, &picker.state, true);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "type to filter   ↑↓ move   Enter use   Esc close",
            Style::default().fg(Color::DarkGray),
        ))
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::ModelPicker;
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
            ..Default::default()
        }
    }

    fn settings() -> Settings {
        let mut settings = Settings::default();
        settings.providers = vec![
            profile(
                "google",
                &["gemini-flash-latest", "gemini-pro-latest"],
                false,
            ),
            profile("groq", &["qwen/qwen3.8-27b", "openai/gpt-oss-120b"], false),
            profile("half-done", &["draft-model"], true),
        ];
        settings.active_provider_id = Some("groq".to_owned());
        settings.model = Some("qwen/qwen3.8-27b".to_owned());
        settings
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn model_picker_groups_by_provider_and_marks_active() {
        let picker = ModelPicker::new(&settings());
        let groups = picker
            .items
            .iter()
            .map(|item| item.group.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(groups[0], "google");
        assert_eq!(groups[2], "groq");
        let marked = picker
            .items
            .iter()
            .position(|item| item.marked)
            .expect("active model marked");
        assert_eq!(
            picker.targets[marked],
            Some((1, "qwen/qwen3.8-27b".to_owned()))
        );
        assert_eq!(
            picker.current_target(),
            Some(&(1, "qwen/qwen3.8-27b".to_owned()))
        );
    }

    #[test]
    fn model_picker_filter_finds_groq_model_by_bare_name() {
        let mut picker = ModelPicker::new(&settings());
        for c in "gpt".chars() {
            picker.state.push_filter(&picker.items, c);
        }
        assert_eq!(
            picker.current_target(),
            Some(&(1, "openai/gpt-oss-120b".to_owned()))
        );
    }

    #[test]
    fn model_picker_tags_free_and_no_tools_models() {
        let mut settings = settings();
        settings.providers[1].model_info.insert(
            "openai/gpt-oss-120b".to_owned(),
            crate::ModelInfo {
                free: Some(true),
                tools: Some(false),
                context: None,
            },
        );
        let picker = ModelPicker::new(&settings);
        let row = picker
            .items
            .iter()
            .find(|item| item.detail.starts_with("openai/gpt-oss-120b"))
            .expect("model row");
        assert!(
            row.detail.contains("free") && row.detail.contains("no tools"),
            "{}",
            row.detail
        );
        let plain = picker
            .items
            .iter()
            .find(|item| item.detail == "qwen/qwen3.8-27b")
            .expect("untagged row");
        assert_eq!(plain.detail, "qwen/qwen3.8-27b");
    }

    #[test]
    fn model_picker_draft_provider_models_are_not_selectable() {
        let picker = ModelPicker::new(&settings());
        let draft = picker
            .items
            .iter()
            .position(|item| item.detail.contains("draft-model"))
            .expect("draft model listed");
        assert!(picker.items[draft].dimmed);
        assert!(!picker.items[draft].selectable);
        assert_eq!(picker.targets[draft], None);
    }

    #[test]
    fn slash_model_without_argument_opens_picker() {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.input = "/model".to_owned();
        app.submit().expect("submit");
        assert!(app.model_picker.is_some());
    }

    #[test]
    fn model_picker_enter_activates_exact_pair_and_closes() {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.model_picker = Some(ModelPicker::new(&app.settings));
        for c in "gpt".chars() {
            app.handle_model_picker_key(key(KeyCode::Char(c)))
                .expect("filter");
        }
        app.handle_model_picker_key(key(KeyCode::Enter))
            .expect("enter");
        assert!(app.model_picker.is_none());
        assert_eq!(app.settings.active_provider_id.as_deref(), Some("groq"));
        assert_eq!(app.settings.model.as_deref(), Some("openai/gpt-oss-120b"));
    }

    #[test]
    fn model_picker_esc_closes_without_changes() {
        let mut app = App::new(settings());
        app.model_picker = Some(ModelPicker::new(&app.settings));
        app.handle_model_picker_key(key(KeyCode::Down))
            .expect("down");
        app.handle_model_picker_key(key(KeyCode::Esc)).expect("esc");
        assert!(app.model_picker.is_none());
        assert_eq!(app.settings.model.as_deref(), Some("qwen/qwen3.8-27b"));
    }
}
