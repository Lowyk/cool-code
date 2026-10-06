use crate::tui::render::forms::draw_open_form;
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::{App, ProviderDraft};
use crate::tui::widgets::list::{ListItem, ListState, draw_list};
use crate::{Settings, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub(super) fn provider_items(settings: &Settings) -> Vec<ListItem> {
    let mut items = settings
        .providers
        .iter()
        .map(|profile| {
            let is_default = settings.default_provider_id.as_deref() == Some(profile.id.as_str());
            let mut tags = Vec::new();
            if is_default {
                tags.push("default");
            }
            if profile.auto_switch {
                tags.push("auto");
            }
            if profile.draft {
                tags.push("draft");
            }
            ListItem {
                label: profile.name.clone(),
                detail: tags.join(" · "),
                group: None,
                dimmed: profile.draft,
                selectable: true,
                marked: is_default,
            }
        })
        .collect::<Vec<_>>();
    items.push(ListItem {
        label: "+ Add provider".to_owned(),
        selectable: true,
        ..ListItem::default()
    });
    items
}

pub(super) fn draw_providers(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    if app.provider_form.is_some() {
        draw_open_form(frame, area, app);
        return;
    }
    let items = provider_items(&app.settings);
    let list_height = (items.len() as u16).min(area.height.saturating_sub(7).max(3));
    let state = ListState {
        selected: view.row,
        filter: String::new(),
    };
    draw_list(
        frame,
        Rect::new(area.x, area.y, area.width, list_height),
        &items,
        &state,
        view.focus == Focus::Content,
    );
    let details_y = area.y + list_height + 1;
    if details_y >= area.bottom() {
        return;
    }
    let details_area = Rect::new(area.x, details_y, area.width, area.bottom() - details_y);
    let lines = match app.settings.providers.get(view.row) {
        Some(profile) => {
            let label = |text: &'static str| Span::styled(text, Style::default().fg(Color::Gray));
            vec![
                Line::from(Span::styled(
                    "─".repeat(area.width as usize),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(vec![
                    label("Adapter   "),
                    Span::raw(profile.adapter.clone()),
                ]),
                Line::from(vec![
                    label("Base URL  "),
                    Span::raw(
                        profile
                            .base_url
                            .clone()
                            .unwrap_or_else(|| "preset default".to_owned()),
                    ),
                ]),
                Line::from(vec![
                    label("API key   "),
                    Span::styled(
                        "•••••••• (OS credential store)",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Line::from(vec![
                    label("Models    "),
                    Span::raw(profile.models.len().max(1).to_string()),
                ]),
            ]
        }
        None => vec![Line::from(Span::styled(
            "Add an API connection from a preset or a custom endpoint.",
            Style::default().fg(Color::DarkGray),
        ))],
    };
    frame.render_widget(Paragraph::new(lines), details_area);
}

impl App {
    pub(super) fn handle_providers_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        let provider_count = self.settings.providers.len();
        let row = view.row.min(provider_count);
        if view.confirm_delete {
            view.confirm_delete = false;
            if key.code == KeyCode::Char('y') {
                self.delete_provider(row)?;
                if let Some(view) = self.settings_view.as_mut() {
                    view.row = view.row.min(self.settings.providers.len());
                }
            } else {
                self.notice = "Deletion cancelled.".to_owned();
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Up => view.row = row.saturating_sub(1),
            KeyCode::Down => view.row = (row + 1).min(provider_count),
            KeyCode::Char('n') => self.open_provider_presets(),
            KeyCode::Enter | KeyCode::Char('e') if row == provider_count => {
                self.open_provider_presets()
            }
            KeyCode::Enter | KeyCode::Char('e') => self.edit_provider(row),
            KeyCode::Char('x') if row < provider_count => view.confirm_delete = true,
            KeyCode::Char('d') if row < provider_count => self.set_default_provider(row)?,
            KeyCode::Char('a') if row < provider_count => self.toggle_provider_auto_switch(row)?,
            _ => {}
        }
        Ok(())
    }

    fn open_provider_presets(&mut self) {
        self.provider_form = Some(ProviderDraft {
            choosing_preset: true,
            existing_id: None,
            preset: 0,
            alias: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            models: Vec::new(),
            focus: 0,
        });
    }

    fn set_default_provider(&mut self, index: usize) -> Result<()> {
        let profile = self.settings.providers[index].clone();
        if profile.draft {
            self.notice = "This provider is a draft; finish its setup first.".to_owned();
            return Ok(());
        }
        self.settings.default_provider_id = Some(profile.id.clone());
        if self.settings.active_provider_id.is_none() {
            self.settings.active_provider_id = Some(profile.id.clone());
            self.settings.provider = Some(profile.adapter.clone());
            self.settings.model = profile
                .models
                .first()
                .map(|model| model.id.clone())
                .or_else(|| (!profile.model.is_empty()).then_some(profile.model.clone()));
            self.settings.base_url = profile.base_url.clone();
            self.settings.api_key_env = None;
        }
        write_settings(&self.settings)?;
        self.notice = format!("{} is now the default provider.", profile.name);
        Ok(())
    }

    fn toggle_provider_auto_switch(&mut self, index: usize) -> Result<()> {
        let profile = &mut self.settings.providers[index];
        if profile.draft {
            self.notice = "Finish this draft before enabling automatic model switching.".to_owned();
            return Ok(());
        }
        profile.auto_switch = !profile.auto_switch;
        self.notice = format!(
            "{} auto-switch {}.",
            profile.name,
            if profile.auto_switch {
                "enabled"
            } else {
                "disabled"
            }
        );
        write_settings(&self.settings)
    }
}

#[cfg(test)]
mod tests {
    use super::provider_items;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{ModelProfile, ProviderProfile, Settings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn profile(id: &str, draft: bool, auto_switch: bool) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: id.to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: format!("{id}-model"),
            models: vec![ModelProfile {
                id: format!("{id}-model"),
                name: String::new(),
            }],
            draft,
            auto_switch,
            base_url: None,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_with(providers: Vec<ProviderProfile>, default: Option<&str>) -> App {
        let mut settings = Settings::default();
        settings.providers = providers;
        settings.default_provider_id = default.map(str::to_owned);
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.open_settings(Section::Providers);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus content");
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(key(code)).expect("key");
    }

    #[test]
    fn providers_section_lists_profiles_with_tags() {
        let mut settings = Settings::default();
        settings.providers = vec![profile("groq", false, true), profile("wip", true, false)];
        settings.default_provider_id = Some("groq".to_owned());
        let items = provider_items(&settings);
        assert_eq!(items.len(), 3);
        assert!(items[0].marked);
        assert!(items[0].detail.contains("default"), "{}", items[0].detail);
        assert!(items[0].detail.contains("auto"), "{}", items[0].detail);
        assert!(items[1].detail.contains("draft"), "{}", items[1].detail);
        assert_eq!(items[2].label, "+ Add provider");
    }

    #[test]
    fn providers_d_sets_default_and_a_toggles_auto_switch() {
        let mut app = app_with(
            vec![
                profile("google", false, false),
                profile("groq", false, false),
            ],
            Some("google"),
        );
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.settings.default_provider_id.as_deref(), Some("groq"));
        press(&mut app, KeyCode::Char('a'));
        assert!(app.settings.providers[1].auto_switch);
        press(&mut app, KeyCode::Char('a'));
        assert!(!app.settings.providers[1].auto_switch);
    }

    #[test]
    fn providers_delete_requires_confirmation() {
        let mut app = app_with(
            vec![
                profile("google", false, false),
                profile("groq", false, false),
            ],
            Some("google"),
        );
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.settings.providers.len(), 2);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.settings.providers.len(), 2);
        assert!(app.settings_view.is_some());
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.settings.providers.len(), 1);
        assert_eq!(app.settings.providers[0].id, "groq");
    }

    #[test]
    fn deleting_last_provider_leaves_no_default() {
        let mut app = app_with(vec![profile("groq", false, true)], Some("groq"));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.settings.providers.is_empty());
        assert_eq!(app.settings.default_provider_id, None);
    }

    #[test]
    fn enter_on_add_row_opens_preset_chooser() {
        let mut app = app_with(vec![profile("groq", false, true)], Some("groq"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(
            app.provider_form
                .as_ref()
                .is_some_and(|form| form.choosing_preset)
        );
    }

    #[test]
    fn shortcut_letters_type_into_open_form() {
        let mut app = app_with(vec![profile("groq", false, true)], Some("groq"));
        press(&mut app, KeyCode::Char('e'));
        let before = app.provider_form.as_ref().expect("form open").alias.clone();
        for c in "dax".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.settings.providers.len(), 1);
        let form = app.provider_form.as_ref().expect("form still open");
        assert_eq!(form.alias, format!("{before}dax"));
    }
}
