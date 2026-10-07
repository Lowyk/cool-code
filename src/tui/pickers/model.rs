use crate::Settings;
use crate::tui::render::centered_rect;
use crate::tui::state::App;
use crate::tui::widgets::tree::{RowKind, TreeState, draw_tree};
use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

pub(in crate::tui) struct ModelPicker {
    pub(in crate::tui) tree: TreeState,
}

impl ModelPicker {
    pub(in crate::tui) fn new(settings: &Settings) -> ModelPicker {
        let mut tree = TreeState::default();
        let rows = tree.rows(settings, true);
        tree.select_active(&rows);
        ModelPicker { tree }
    }

    /// The model under the cursor, when the cursor is on a usable model row.
    #[cfg(test)]
    pub(in crate::tui) fn current_target(
        &self,
        settings: &Settings,
    ) -> Option<crate::tui::widgets::tree::ModelTarget> {
        let rows = self.tree.rows(settings, true);
        self.tree.current(&rows).and_then(|row| row.target.clone())
    }
}

impl App {
    pub(in crate::tui) fn handle_model_picker_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(picker) = self.model_picker.as_mut() else {
            return Ok(());
        };
        let rows = picker.tree.rows(&self.settings, true);
        match key.code {
            KeyCode::Up => picker.tree.move_by(&rows, -1),
            KeyCode::Down => picker.tree.move_by(&rows, 1),
            KeyCode::Right => picker.tree.expand(&rows),
            KeyCode::Left => picker.tree.collapse(&rows),
            KeyCode::Backspace => picker.tree.pop_filter(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.tree.push_filter(c)
            }
            KeyCode::Enter => match picker.tree.current(&rows).cloned() {
                Some(row) if row.kind == RowKind::Model => {
                    if let Some(target) = row.target {
                        self.model_picker = None;
                        self.activate_model(target.provider, &target.id)?;
                    }
                }
                Some(_) => picker.tree.toggle(&rows),
                None => {}
            },
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
    settings: &Settings,
) {
    let popup = centered_rect(72, 76, area);
    frame.render_widget(Clear, popup);
    let accent = crate::tui::theme::accent_bright();
    let block = Block::default()
        .title(" Model ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(crate::tui::theme::panel()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 4 {
        return;
    }
    let filter = Line::from(vec![
        Span::styled("> ", Style::default().fg(accent)),
        Span::raw(picker.tree.filter.clone()),
        Span::styled("_", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(
        Paragraph::new(filter),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 3);
    let rows = picker.tree.rows(settings, true);
    draw_tree(frame, list_area, &rows, picker.tree.selected, true);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "type to filter   ↑↓ move   ←→ fold   Enter use or fold   Esc close",
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
    use crate::tui::widgets::tree::{RowKind, TreeRow};
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

    fn rows(picker: &ModelPicker, settings: &Settings) -> Vec<TreeRow> {
        picker.tree.rows(settings, true)
    }

    #[test]
    fn the_picker_opens_on_the_active_model_inside_its_provider() {
        let settings = settings();
        let picker = ModelPicker::new(&settings);
        let rows = rows(&picker, &settings);
        assert_eq!(rows[0].label, "google");
        assert!(!rows[0].open, "inactive providers start folded");
        let groq = rows.iter().position(|row| row.label == "groq").unwrap();
        assert!(rows[groq].open && rows[groq].marked);
        let target = picker.current_target(&settings).expect("on a model");
        assert_eq!(
            (target.provider, target.id.as_str()),
            (1, "qwen/qwen3.8-27b")
        );
    }

    #[test]
    fn the_picker_filter_finds_a_model_by_bare_name() {
        let settings = settings();
        let mut picker = ModelPicker::new(&settings);
        for c in "gpt".chars() {
            picker.tree.push_filter(c);
        }
        let target = picker.current_target(&settings).expect("match");
        assert_eq!(
            (target.provider, target.id.as_str()),
            (1, "openai/gpt-oss-120b")
        );
    }

    #[test]
    fn the_picker_tags_free_and_no_tools_models() {
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
        let rows = rows(&picker, &settings);
        let tagged = rows
            .iter()
            .find(|row| row.label.contains("openai/gpt-oss-120b"))
            .expect("tagged row");
        assert!(
            tagged.detail.contains("free") && tagged.detail.contains("no tools"),
            "{}",
            tagged.detail
        );
        let plain = rows
            .iter()
            .find(|row| row.label.contains("qwen/qwen3.8-27b"))
            .expect("untagged row");
        assert_eq!(plain.detail, "");
    }

    #[test]
    fn draft_providers_are_listed_dimmed_and_cannot_be_chosen() {
        let settings = settings();
        let mut picker = ModelPicker::new(&settings);
        let all = rows(&picker, &settings);
        let draft = all.iter().position(|row| row.label == "half-done").unwrap();
        assert!(all[draft].dimmed);
        picker.tree.selected = draft;
        picker.tree.expand(&all);
        let opened = rows(&picker, &settings);
        let model = opened
            .iter()
            .find(|row| row.label.contains("draft-model"))
            .expect("draft model listed");
        assert!(model.dimmed && model.target.is_none());
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
    fn enter_on_a_model_activates_the_exact_pair_and_closes() {
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
    fn enter_on_a_provider_folds_it_instead_of_choosing() {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.model_picker = Some(ModelPicker::new(&app.settings));
        app.handle_model_picker_key(key(KeyCode::Left))
            .expect("climb to the creator");
        app.handle_model_picker_key(key(KeyCode::Left))
            .expect("fold it");
        // Selection climbed to the "Qwen" creator, then folded it away.
        app.handle_model_picker_key(key(KeyCode::Enter))
            .expect("enter");
        assert!(app.model_picker.is_some(), "a heading is not a model");
        assert_eq!(app.settings.model.as_deref(), Some("qwen/qwen3.8-27b"));
        let picker = app.model_picker.as_ref().unwrap();
        let rows = picker.tree.rows(&app.settings, true);
        assert!(
            rows.iter()
                .any(|row| row.kind == RowKind::Creator && row.open)
        );
    }

    #[test]
    fn right_unfolds_a_provider_to_reach_its_models() {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.model_picker = Some(ModelPicker::new(&app.settings));
        for _ in 0..10 {
            app.handle_model_picker_key(key(KeyCode::Up)).expect("up");
        }
        app.handle_model_picker_key(key(KeyCode::Right))
            .expect("unfold google");
        app.handle_model_picker_key(key(KeyCode::Down))
            .expect("down");
        app.handle_model_picker_key(key(KeyCode::Enter))
            .expect("choose");
        assert_eq!(app.settings.active_provider_id.as_deref(), Some("google"));
        assert_eq!(app.settings.model.as_deref(), Some("gemini-flash-latest"));
    }

    #[test]
    fn esc_closes_without_changes() {
        let mut app = App::new(settings());
        app.model_picker = Some(ModelPicker::new(&app.settings));
        app.handle_model_picker_key(key(KeyCode::Down))
            .expect("down");
        app.handle_model_picker_key(key(KeyCode::Esc)).expect("esc");
        assert!(app.model_picker.is_none());
        assert_eq!(app.settings.model.as_deref(), Some("qwen/qwen3.8-27b"));
    }
}
