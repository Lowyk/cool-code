use crate::tui::render::forms::draw_open_form;
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::{App, ChainDraft};
use crate::tui::widgets::list::{ListItem, ListState, draw_list};
use crate::{Settings, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;

pub(super) fn chain_items(settings: &Settings) -> Vec<ListItem> {
    let mut items = settings
        .model_chains
        .iter()
        .map(|chain| {
            let count = chain.members.len();
            ListItem {
                label: if chain.alias.is_empty() {
                    chain.id.clone()
                } else {
                    chain.alias.clone()
                },
                detail: format!(
                    "{} · {count} model{} · auto-on-select {}",
                    chain.id,
                    if count == 1 { "" } else { "s" },
                    if chain.activate_on_select {
                        "on"
                    } else {
                        "off"
                    }
                ),
                selectable: true,
                marked: settings.active_chain_id.as_deref() == Some(chain.id.as_str()),
                ..ListItem::default()
            }
        })
        .collect::<Vec<_>>();
    items.push(ListItem {
        label: "+ New chain".to_owned(),
        selectable: true,
        ..ListItem::default()
    });
    items
}

pub(super) fn draw_auto_switch(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    if app.chain_form.is_some() {
        draw_open_form(frame, area, app);
        return;
    }
    let state = ListState {
        selected: view.row,
        filter: String::new(),
    };
    draw_list(
        frame,
        area,
        &chain_items(&app.settings),
        &state,
        view.focus == Focus::Content,
    );
}

impl App {
    pub(super) fn handle_auto_switch_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        let count = self.settings.model_chains.len();
        let row = view.row.min(count);
        if view.confirm_delete {
            view.confirm_delete = false;
            if key.code == KeyCode::Char('y') && row < count {
                let id = self.settings.model_chains.remove(row).id;
                if self.settings.active_chain_id.as_deref() == Some(id.as_str()) {
                    self.settings.active_chain_id = None;
                }
                write_settings(&self.settings)?;
                self.notice = format!("Chain `{id}` removed.");
            } else {
                self.notice = "Deletion cancelled.".to_owned();
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Up => view.row = row.saturating_sub(1),
            KeyCode::Down => view.row = (row + 1).min(count),
            KeyCode::Char('x') if row < count => view.confirm_delete = true,
            KeyCode::Char('n') => self.open_new_chain_form(),
            KeyCode::Enter | KeyCode::Char('e') if row == count => self.open_new_chain_form(),
            KeyCode::Enter | KeyCode::Char('e') => self.edit_chain(row),
            KeyCode::Char(' ') if row < count => {
                let id = self.settings.model_chains[row].id.clone();
                if self.settings.active_chain_id.as_deref() == Some(id.as_str()) {
                    self.settings.active_chain_id = None;
                    write_settings(&self.settings)?;
                    self.notice = format!("Chain `{id}` deactivated.");
                } else {
                    self.activate_chain(&id)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn open_new_chain_form(&mut self) {
        self.chain_form = Some(ChainDraft {
            original_id: None,
            alias: String::new(),
            id: String::new(),
            members: Vec::new(),
            activate_on_select: false,
            focus: 0,
            member_index: 0,
            picking_member: false,
            candidate_index: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::chain_items;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::{ChainModel, ModelChain, ModelProfile, ProviderProfile, Settings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn settings() -> Settings {
        let mut settings = Settings::default();
        settings.providers = vec![ProviderProfile {
            id: "groq".to_owned(),
            name: "groq".to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: "qwen".to_owned(),
            models: vec![ModelProfile {
                id: "qwen".to_owned(),
                name: String::new(),
            }],
            draft: false,
            auto_switch: true,
            base_url: None,
        }];
        settings.model_chains = vec![ModelChain {
            id: "fast".to_owned(),
            alias: "Fast".to_owned(),
            members: vec![ChainModel {
                provider_id: "groq".to_owned(),
                model_id: "qwen".to_owned(),
            }],
            activate_on_select: false,
        }];
        settings
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn app() -> App {
        let mut app = App::new(settings());
        app.trust_prompt = false;
        app.open_settings(Section::AutoSwitch);
        press(&mut app, KeyCode::Right);
        app
    }

    #[test]
    fn auto_switch_lists_chains_with_new_row() {
        let items = chain_items(&settings());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Fast");
        assert!(items[0].detail.contains("1 model"), "{}", items[0].detail);
        assert_eq!(items[1].label, "+ New chain");
    }

    #[test]
    fn space_toggles_the_active_chain() {
        let mut app = app();
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.settings.active_chain_id.as_deref(), Some("fast"));
        assert!(chain_items(&app.settings)[0].marked);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.settings.active_chain_id, None);
    }

    #[test]
    fn enter_edits_a_chain_and_opens_new_chain_form_on_last_row() {
        let mut app = app();
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.chain_form
                .as_ref()
                .and_then(|form| form.original_id.clone()),
            Some("fast".to_owned())
        );
        app.chain_form = None;
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(
            app.chain_form
                .as_ref()
                .is_some_and(|form| form.original_id.is_none())
        );
    }

    #[test]
    fn chain_delete_requires_confirmation() {
        let mut app = app();
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.settings.model_chains.len(), 1);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.settings.model_chains.is_empty());
    }
}
