use crate::endpoints::describe_key;
use crate::tui::forms::preset_for_profile;
use crate::tui::render::forms::draw_open_form;
use crate::tui::settings::sync::{LimitsEntry, LimitsState, load_key};
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::{App, PROVIDER_PRESETS, ProviderDraft};
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
    let list_height = (items.len() as u16).min(area.height.saturating_sub(15).max(3));
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
            let mut lines = vec![
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
                        match app.key_shapes.get(&profile.id) {
                            Some(shape) => format!("•••••••• OS credential store · {shape}"),
                            None => "•••••••• (OS credential store)".to_owned(),
                        },
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                models_line(app, profile),
            ];
            if profile.limits_url.is_some() {
                lines.extend(usage_lines(app.limits.get(&profile.id)));
            }
            lines
        }
        None => vec![Line::from(Span::styled(
            "Add an API connection from a preset or a custom endpoint.",
            Style::default().fg(Color::DarkGray),
        ))],
    };
    frame.render_widget(Paragraph::new(lines), details_area);
}

fn models_line(app: &App, profile: &crate::ProviderProfile) -> Line<'static> {
    let mut text = profile.models.len().max(1).to_string();
    if profile.models_url.is_some() {
        let free = profile
            .model_info
            .values()
            .filter(|info| info.free == Some(true))
            .count();
        if free > 0 {
            text.push_str(&format!(" · {free} free"));
        }
        if app.models_loading.contains(&profile.id) {
            text.push_str(" · loading…");
        } else {
            text.push_str(" · f refreshes from the endpoint");
        }
    }
    Line::from(vec![
        Span::styled("Models    ", Style::default().fg(Color::Gray)),
        Span::raw(text),
    ])
}

fn remaining_bar(fraction: f32) -> Span<'static> {
    let filled = (fraction.clamp(0.0, 1.0) * 12.0).round() as usize;
    let color = if fraction > 0.5 {
        Color::Rgb(110, 220, 130)
    } else if fraction > 0.2 {
        Color::Rgb(240, 210, 90)
    } else {
        Color::Rgb(235, 80, 80)
    };
    Span::styled(
        format!("{}{}", "█".repeat(filled), "░".repeat(12 - filled)),
        Style::default().fg(color),
    )
}

fn usage_lines(entry: Option<&LimitsEntry>) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::Gray);
    let dim = Style::default().fg(Color::DarkGray);
    let header = |text: String| {
        Line::from(vec![
            Span::styled("Usage     ", label),
            Span::styled(text, dim),
        ])
    };
    match entry.map(|entry| &entry.state) {
        None => vec![header("not loaded yet · u loads it".to_owned())],
        Some(LimitsState::Loading) => vec![header("loading…".to_owned())],
        Some(LimitsState::Failed(error)) => {
            let shown: String = error.chars().take(90).collect();
            vec![header(format!("unavailable: {shown}"))]
        }
        Some(LimitsState::Ready(lines)) => {
            let mut out = vec![header("u refreshes".to_owned())];
            for line in lines {
                let mut spans = vec![
                    Span::styled(format!("  {:<9}", line.label), label),
                    Span::raw(line.value.clone()),
                ];
                if let Some(fraction) = line.remaining {
                    spans.push(Span::raw("  "));
                    spans.push(remaining_bar(fraction));
                }
                out.push(Line::from(spans));
            }
            out
        }
    }
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
            KeyCode::Char('f') if row < provider_count => self.start_models_fetch(row, false),
            KeyCode::Char('u') if row < provider_count => self.start_limits_fetch(row, true),
            _ => {}
        }
        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            // Selecting a provider loads its usage once; fresh results are reused for a minute.
            let selected = self.settings_view.as_ref().map_or(0, |view| view.row);
            self.refresh_key_shape(selected, false);
            self.start_limits_fetch(selected, false);
        }
        Ok(())
    }

    /// Measures the saved key (length, quotes, spaces, expected prefix) so the details pane can
    /// show why a provider might reject it, without ever displaying the key.
    pub(in crate::tui) fn refresh_key_shape(&mut self, index: usize, force: bool) {
        let Some(profile) = self.settings.providers.get(index) else {
            return;
        };
        if !force && self.key_shapes.contains_key(&profile.id) {
            return;
        }
        let id = profile.id.clone();
        let prefix = PROVIDER_PRESETS[preset_for_profile(profile)].key_prefix;
        let shape = match load_key(&id, false) {
            Some(key) => describe_key(&key, prefix),
            None => "no key saved".to_owned(),
        };
        self.key_shapes.insert(id, shape);
    }

    fn open_provider_presets(&mut self) {
        self.provider_form = Some(ProviderDraft {
            choosing_preset: true,
            existing_id: None,
            preset: 0,
            alias: String::new(),
            suggested_alias: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            models: Vec::new(),
            models_endpoint: String::new(),
            limits_endpoint: String::new(),
            managed_models: false,
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
            ..Default::default()
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

    fn with_endpoints(mut profile: ProviderProfile) -> ProviderProfile {
        profile.base_url = Some("https://api.example.com/v1".to_owned());
        profile.models_url = Some("https://api.example.com/v1/models".to_owned());
        profile.limits_url = Some("https://api.example.com/v1/limits".to_owned());
        profile
    }

    fn screen(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 40)).expect("terminal");
        terminal
            .draw(|frame| crate::tui::render::draw(frame, app, 0))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn selecting_a_provider_loads_its_usage_once() {
        let mut app = app_with(
            vec![
                with_endpoints(profile("p1", false, false)),
                with_endpoints(profile("p2", false, false)),
                profile("p3", false, false),
            ],
            None,
        );
        assert_eq!(
            app.spawned_tasks, 1,
            "entering the section loads the first provider"
        );
        press(&mut app, KeyCode::Down);
        assert_eq!(app.spawned_tasks, 2);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.spawned_tasks, 2, "already loading or fresh");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            app.spawned_tasks, 2,
            "a provider without a limits endpoint fetches nothing"
        );
    }

    #[test]
    fn f_loads_models_and_u_refreshes_usage_for_the_selected_provider() {
        let mut app = app_with(vec![with_endpoints(profile("p1", false, false))], None);
        let before = app.spawned_tasks;
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.spawned_tasks, before + 1);
        assert!(app.models_loading.contains("p1"));
        app.apply_task_result(crate::tui::settings::sync::TaskResult::Limits {
            provider_id: "p1".to_owned(),
            url: "https://api.example.com/v1/limits".to_owned(),
            result: Ok(Vec::new()),
        });
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.spawned_tasks, before + 2);
        let mut plain = app_with(vec![profile("p2", false, false)], None);
        press(&mut plain, KeyCode::Char('f'));
        assert!(
            plain.notice.contains("no models endpoint"),
            "{}",
            plain.notice
        );
    }

    #[test]
    fn the_details_pane_shows_usage_with_a_remaining_bar() {
        use crate::endpoints::LimitLine;
        use crate::tui::settings::sync::{LimitsEntry, LimitsState};
        let mut app = app_with(vec![with_endpoints(profile("p1", false, false))], None);
        let line = |label: &str, value: &str, remaining| LimitLine {
            label: label.to_owned(),
            value: value.to_owned(),
            remaining,
        };
        app.limits.insert(
            "p1".to_owned(),
            LimitsEntry {
                fetched_at: std::time::Instant::now(),
                state: LimitsState::Ready(vec![
                    line("Balance", "3,000,000 tokens", None),
                    line("Weekly", "250,000 / 1,000,000 tokens", Some(0.75)),
                ]),
            },
        );
        let shown = screen(&app);
        assert!(shown.contains("3,000,000 tokens"), "{shown}");
        assert!(shown.contains("Weekly") && shown.contains('█'), "{shown}");
        app.limits.get_mut("p1").unwrap().state =
            LimitsState::Failed("provider returned 401".to_owned());
        assert!(screen(&app).contains("unavailable: provider returned 401"));
        app.limits.get_mut("p1").unwrap().state = LimitsState::Loading;
        assert!(screen(&app).contains("loading…"));
    }

    #[test]
    fn the_models_line_counts_free_models() {
        let mut provider = with_endpoints(profile("p1", false, false));
        provider.model_info.insert(
            "p1-model".to_owned(),
            crate::ModelInfo {
                free: Some(true),
                ..Default::default()
            },
        );
        let app = app_with(vec![provider], None);
        assert!(screen(&app).contains("1 free"));
    }

    #[test]
    fn the_details_pane_describes_the_saved_keys_shape_without_showing_it() {
        let mut multiai = with_endpoints(profile("p1", false, false));
        multiai.base_url = Some("https://multiai.store/v1".to_owned());
        multiai.models_url = Some("https://multiai.store/v1/models".to_owned());
        multiai.limits_url = Some("https://multiai.store/v1/subscription/limits".to_owned());
        let app = app_with(vec![multiai], None);
        let shown = screen(&app);
        // In tests the stored key is the 8-character stand-in "test-key".
        assert!(shown.contains("8 characters"), "{shown}");
        assert!(shown.contains("does not start with ma-live-"), "{shown}");
        assert!(
            !shown.contains("test-key"),
            "the key itself must never be shown"
        );
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
