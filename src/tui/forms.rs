use crate::tui::models::{available_chain_models, model_name, slug};
use crate::tui::state::{
    App, ChainDraft, ModelDraft, PROVIDER_PRESETS, ProviderDraft, SettingsTab,
    adjacent_settings_tab, edit_string,
};
use crate::{
    ModelChain, ModelProfile, ProviderProfile, Settings, provider, secrets, write_settings,
};
use anyhow::{Context, Result};
use crossterm::event::{self, KeyCode};

pub(super) fn unique_provider_alias(providers: &[ProviderProfile], base: &str) -> String {
    if !providers
        .iter()
        .any(|profile| profile.name.eq_ignore_ascii_case(base))
    {
        return base.to_owned();
    }
    for suffix in 2usize.. {
        let candidate = format!("{base} {suffix}");
        if !providers
            .iter()
            .any(|profile| profile.name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
    }
    unreachable!("a provider alias suffix is available")
}

pub(super) fn remove_provider_profile(settings: &mut Settings, id: &str) -> bool {
    let Some(index) = settings
        .providers
        .iter()
        .position(|profile| profile.id == id)
    else {
        return false;
    };
    settings.providers.remove(index);

    for chain in &mut settings.model_chains {
        chain.members.retain(|member| member.provider_id != id);
    }
    settings
        .model_chains
        .retain(|chain| !chain.members.is_empty());
    if settings
        .active_chain_id
        .as_deref()
        .is_some_and(|id| !settings.model_chains.iter().any(|chain| chain.id == id))
    {
        settings.active_chain_id = None;
    }

    let default_is_invalid = settings
        .default_provider_id
        .as_deref()
        .is_some_and(|default_id| {
            !settings
                .providers
                .iter()
                .any(|profile| profile.id == default_id && !profile.draft)
        });
    if settings.default_provider_id.as_deref() == Some(id) || default_is_invalid {
        settings.default_provider_id = settings
            .providers
            .iter()
            .find(|profile| !profile.draft)
            .map(|profile| profile.id.clone());
    }

    if settings.active_provider_id.as_deref() == Some(id) {
        let replacement = settings
            .default_provider_id
            .as_deref()
            .and_then(|default_id| {
                settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == default_id)
            })
            .or_else(|| settings.providers.iter().find(|profile| !profile.draft))
            .cloned();
        if let Some(profile) = replacement {
            if settings.default_provider_id.is_none() {
                settings.default_provider_id = Some(profile.id.clone());
            }
            settings.active_provider_id = Some(profile.id.clone());
            settings.provider = Some(profile.adapter.clone());
            settings.model = profile
                .models
                .first()
                .map(|model| model.id.clone())
                .or_else(|| (!profile.model.is_empty()).then_some(profile.model.clone()));
            settings.base_url = profile.base_url.clone();
            settings.api_key_env = None;
        } else {
            settings.active_provider_id = None;
            settings.provider = None;
            settings.model = None;
            settings.base_url = None;
            settings.api_key_env = None;
        }
    }
    true
}

pub(super) fn provider_focus_layout(
    form: &ProviderDraft,
) -> (usize, usize, usize, usize, usize, usize) {
    let custom = PROVIDER_PRESETS[form.preset].custom;
    let key_focus = if custom { 2 } else { 1 };
    let model_start = key_focus + 1;
    let create_focus = model_start + form.models.len() * 3;
    let save_focus = create_focus + 1;
    let draft_focus = save_focus + 1;
    let cancel_focus = draft_focus + 1;
    (
        key_focus,
        model_start,
        create_focus,
        save_focus,
        draft_focus,
        cancel_focus,
    )
}

impl App {
    pub(super) fn edit_provider(&mut self, provider_index: usize) {
        let Some(profile) = self.settings.providers.get(provider_index) else {
            return;
        };
        let preset = if profile.name.to_ascii_lowercase().contains("openrouter") {
            3
        } else {
            match profile.adapter.as_str() {
                "openai" => 0,
                "anthropic" => 1,
                "google" => 2,
                "anthropic-compatible" => 4,
                _ => 5,
            }
        };
        let models = if profile.models.is_empty() && !profile.model.is_empty() {
            vec![ModelDraft {
                id: profile.model.clone(),
                name: model_name(&profile.model),
            }]
        } else {
            profile
                .models
                .iter()
                .map(|model| ModelDraft {
                    id: model.id.clone(),
                    name: model.name.clone(),
                })
                .collect()
        };
        self.provider_form = Some(ProviderDraft {
            choosing_preset: false,
            existing_id: Some(profile.id.clone()),
            preset,
            alias: profile.name.clone(),
            base_url: profile.base_url.clone().unwrap_or_default(),
            api_key: String::new(),
            models,
            focus: 0,
        });
    }

    pub(super) fn delete_provider(&mut self, provider_index: usize) -> Result<()> {
        let Some(profile) = self.settings.providers.get(provider_index).cloned() else {
            return Ok(());
        };
        let previous_settings = self.settings.clone();
        remove_provider_profile(&mut self.settings, &profile.id);
        self.provider_index = self
            .provider_index
            .min(self.settings.providers.len().saturating_sub(1));
        if let Err(error) = write_settings(&self.settings) {
            self.settings = previous_settings;
            return Err(error);
        }
        if let Err(error) = secrets::delete(&profile.id) {
            self.settings = previous_settings;
            if let Err(restore_error) = write_settings(&self.settings) {
                return Err(error).context(format!(
                    "restoring provider settings after credential deletion failed: {restore_error:#}"
                ));
            }
            return Err(error);
        }
        self.notice = format!("{} and its saved API key were deleted.", profile.name);
        Ok(())
    }

    pub(super) fn save_provider(&mut self, as_draft: bool) -> Result<()> {
        let Some(draft) = self.provider_form.as_ref() else {
            return Ok(());
        };
        let name = draft.alias.trim().to_owned();
        let api_key = draft.api_key.trim().to_owned();
        let preset = &PROVIDER_PRESETS[draft.preset];
        let models = draft
            .models
            .iter()
            .filter(|model| !model.id.trim().is_empty())
            .map(|model| ModelProfile {
                id: model.id.trim().to_owned(),
                name: if model.name.trim().is_empty() {
                    model_name(&model.id)
                } else {
                    model.name.trim().to_owned()
                },
            })
            .collect::<Vec<_>>();
        if name.is_empty() {
            self.notice = "Add an alias for this provider.".to_owned();
            return Ok(());
        }
        if preset.custom && draft.base_url.trim().is_empty() {
            self.notice = "A base URL is required for a custom API provider.".to_owned();
            return Ok(());
        }
        if !as_draft && models.is_empty() {
            self.notice = "Add at least one model ID before saving an enabled provider.".to_owned();
            return Ok(());
        }
        let existing_key = draft
            .existing_id
            .as_deref()
            .map(secrets::load)
            .transpose()?
            .flatten();
        if !as_draft && api_key.is_empty() && existing_key.is_none() {
            self.notice = "Add an API key, or save this provider as a draft.".to_owned();
            return Ok(());
        }
        if self.settings.providers.iter().any(|profile| {
            Some(profile.id.as_str()) != draft.existing_id.as_deref()
                && profile.name.eq_ignore_ascii_case(&name)
        }) {
            self.notice = "A provider with that name already exists.".to_owned();
            return Ok(());
        }
        let id = draft
            .existing_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let default_model = models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default();
        let base_url = if preset.custom {
            (!draft.base_url.trim().is_empty())
                .then(|| draft.base_url.trim().trim_end_matches('/').to_owned())
        } else {
            preset.base_url.map(str::to_owned)
        };
        let existing_auto_switch = !as_draft
            && self
                .settings
                .providers
                .iter()
                .find(|profile| profile.id == id)
                .is_some_and(|profile| profile.auto_switch);
        let adapter = self
            .settings
            .providers
            .iter()
            .find(|profile| profile.id == id && profile.adapter == "groq")
            .map(|_| "groq")
            .unwrap_or(preset.adapter);
        let profile = ProviderProfile {
            id: id.clone(),
            name: name.clone(),
            adapter: adapter.to_owned(),
            model: default_model.clone(),
            models,
            draft: as_draft,
            auto_switch: existing_auto_switch,
            base_url: base_url.clone(),
        };

        if !api_key.is_empty() {
            secrets::store(&id, &api_key)?;
        }
        let previous_settings = self.settings.clone();
        if let Some(index) = self
            .settings
            .providers
            .iter()
            .position(|profile| profile.id == id)
        {
            self.settings.providers[index] = profile;
        } else {
            self.settings.providers.push(profile);
        }
        if !as_draft {
            if self.settings.active_provider_id.as_deref() == Some(id.as_str()) {
                self.settings.provider = Some(adapter.to_owned());
                self.settings.model = Some(default_model);
                self.settings.base_url = base_url;
                self.settings.api_key_env = None;
            }
        } else {
            if self.settings.active_provider_id.as_deref() == Some(id.as_str()) {
                self.settings.active_provider_id = None;
                self.settings.model = None;
                self.settings.provider = None;
                self.settings.base_url = None;
            }
            if self.settings.default_provider_id.as_deref() == Some(id.as_str()) {
                self.settings.default_provider_id = None;
            }
        }
        self.provider_index = self
            .settings
            .providers
            .iter()
            .position(|profile| profile.id == id)
            .unwrap_or(0);
        if let Err(error) = write_settings(&self.settings) {
            self.settings = previous_settings;
            if !api_key.is_empty() {
                if let Some(existing_key) = existing_key {
                    let _ = secrets::store(&id, &existing_key);
                } else {
                    let _ = secrets::delete(&id);
                }
            }
            return Err(error);
        }
        self.provider_form = None;
        self.settings_menu = false;
        self.notice = if as_draft {
            format!("{name} saved as a draft.")
        } else {
            format!(
                "{name} ({}) saved. Press Enter to make it default or Space to enable automatic switching.",
                preset.label
            )
        };
        Ok(())
    }

    pub(super) fn handle_provider_form(&mut self, key: event::KeyEvent) -> Result<()> {
        let choosing_preset = self
            .provider_form
            .as_ref()
            .is_some_and(|form| form.choosing_preset);
        if choosing_preset {
            let Some(form) = self.provider_form.as_mut() else {
                return Ok(());
            };
            match key.code {
                KeyCode::Esc => self.provider_form = None,
                KeyCode::Up | KeyCode::Left => form.preset = form.preset.saturating_sub(1),
                KeyCode::Down | KeyCode::Right => {
                    form.preset = (form.preset + 1).min(PROVIDER_PRESETS.len() - 1)
                }
                KeyCode::Enter => {
                    let preset = &PROVIDER_PRESETS[form.preset];
                    form.choosing_preset = false;
                    form.alias = unique_provider_alias(&self.settings.providers, preset.label);
                    form.base_url = preset.base_url.unwrap_or_default().to_owned();
                    form.models = preset
                        .models
                        .iter()
                        .map(|(id, name)| ModelDraft {
                            id: (*id).to_owned(),
                            name: (*name).to_owned(),
                        })
                        .collect();
                    form.focus = 0;
                }
                _ => {}
            }
            return Ok(());
        }
        if key.code == KeyCode::Esc {
            self.provider_form = None;
            return Ok(());
        }
        let focus = self
            .provider_form
            .as_ref()
            .map(|form| form.focus)
            .unwrap_or(0);
        let (key_focus, model_start, create_focus, save_focus, draft_focus, cancel_focus) = {
            let form = self.provider_form.as_ref().expect("provider form active");
            provider_focus_layout(form)
        };
        if matches!(key.code, KeyCode::Tab | KeyCode::Down | KeyCode::Right) {
            let next = if focus >= cancel_focus { 0 } else { focus + 1 };
            self.provider_form
                .as_mut()
                .expect("provider form active")
                .focus = next;
            return Ok(());
        }
        if matches!(key.code, KeyCode::Up | KeyCode::Left) {
            let previous = focus.saturating_sub(1);
            self.provider_form
                .as_mut()
                .expect("provider form active")
                .focus = previous;
            return Ok(());
        }
        if key.code == KeyCode::Enter {
            if focus == create_focus {
                let form = self.provider_form.as_mut().expect("provider form active");
                form.models.push(ModelDraft {
                    id: String::new(),
                    name: String::new(),
                });
                form.focus = model_start + (form.models.len() - 1) * 3;
            } else if focus == save_focus {
                self.save_provider(false)?;
            } else if focus == draft_focus {
                self.save_provider(true)?;
            } else if focus == cancel_focus {
                self.provider_form = None;
            } else if focus >= model_start && focus < create_focus && (focus - model_start) % 3 == 2
            {
                let row = (focus - model_start) / 3;
                let form = self.provider_form.as_mut().expect("provider form active");
                if row < form.models.len() {
                    form.models.remove(row);
                }
                form.focus = focus.min(provider_focus_layout(form).5);
            } else {
                let form = self.provider_form.as_mut().expect("provider form active");
                form.focus = (focus + 1).min(cancel_focus);
            }
            return Ok(());
        }
        if let Some(form) = self.provider_form.as_mut() {
            if focus == 0 {
                edit_string(&mut form.alias, key);
            } else if PROVIDER_PRESETS[form.preset].custom && focus == 1 {
                edit_string(&mut form.base_url, key);
            } else if focus == key_focus {
                edit_string(&mut form.api_key, key);
            } else if focus >= model_start && focus < create_focus {
                let row = (focus - model_start) / 3;
                let column = (focus - model_start) % 3;
                if let Some(model) = form.models.get_mut(row) {
                    match column {
                        0 => edit_string(&mut model.id, key),
                        1 => edit_string(&mut model.name, key),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn handle_settings_key(&mut self, key: event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.settings_menu = false,
            KeyCode::Tab | KeyCode::Right => {
                self.settings_tab = adjacent_settings_tab(self.settings_tab, true)
            }
            KeyCode::Left => self.settings_tab = adjacent_settings_tab(self.settings_tab, false),
            KeyCode::Char('n') if self.settings_tab == SettingsTab::Providers => {
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
            KeyCode::Char('n') if self.settings_tab == SettingsTab::AutoSwitch => {
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
            KeyCode::Up if self.settings_tab == SettingsTab::Providers => {
                self.provider_index = self.provider_index.saturating_sub(1);
            }
            KeyCode::Char('e') if self.settings_tab == SettingsTab::Providers => {
                self.edit_provider(self.provider_index);
            }
            KeyCode::Char('d') if self.settings_tab == SettingsTab::Providers => {
                self.delete_provider(self.provider_index)?;
            }
            KeyCode::Char('e') if self.settings_tab == SettingsTab::AutoSwitch => {
                self.edit_chain(self.chain_index);
            }
            KeyCode::Char('d') if self.settings_tab == SettingsTab::AutoSwitch => {
                if let Some(chain) = self.settings.model_chains.get(self.chain_index) {
                    let id = chain.id.clone();
                    self.settings.model_chains.remove(self.chain_index);
                    if self.settings.active_chain_id.as_deref() == Some(&id) {
                        self.settings.active_chain_id = None;
                    }
                    self.chain_index = self
                        .chain_index
                        .min(self.settings.model_chains.len().saturating_sub(1));
                    write_settings(&self.settings)?;
                    self.notice = format!("Chain `{id}` removed.");
                }
            }
            KeyCode::Up if self.settings_tab == SettingsTab::AutoSwitch => {
                self.chain_index = self.chain_index.saturating_sub(1);
            }
            KeyCode::Down if self.settings_tab == SettingsTab::AutoSwitch => {
                self.chain_index =
                    (self.chain_index + 1).min(self.settings.model_chains.len().saturating_sub(1));
            }
            KeyCode::Enter if self.settings_tab == SettingsTab::AutoSwitch => {
                if let Some(chain) = self.settings.model_chains.get(self.chain_index) {
                    let id = chain.id.clone();
                    self.activate_chain(&id)?;
                }
            }
            KeyCode::Down if self.settings_tab == SettingsTab::Providers => {
                self.provider_index =
                    (self.provider_index + 1).min(self.settings.providers.len().saturating_sub(1));
            }
            KeyCode::Enter if self.settings_tab == SettingsTab::Providers => {
                if let Some(profile) = self.settings.providers.get(self.provider_index).cloned() {
                    if profile.draft {
                        self.notice =
                            "This provider is a draft; finish its setup before activating it."
                                .to_owned();
                    } else {
                        self.settings.default_provider_id = Some(profile.id.clone());
                        if self.settings.active_provider_id.is_none() {
                            self.settings.active_provider_id = Some(profile.id.clone());
                            self.settings.provider = Some(profile.adapter.clone());
                            self.settings.model = profile
                                .models
                                .first()
                                .map(|model| model.id.clone())
                                .or_else(|| {
                                    (!profile.model.is_empty()).then_some(profile.model.clone())
                                });
                            self.settings.base_url = profile.base_url.clone();
                            self.settings.api_key_env = None;
                        }
                        write_settings(&self.settings)?;
                        self.notice = format!(
                            "{} is now the default provider; auto-switch activation is toggled with Space.",
                            profile.name
                        );
                    }
                }
            }
            KeyCode::Char(' ') if self.settings_tab == SettingsTab::Providers => {
                if let Some(profile) = self.settings.providers.get_mut(self.provider_index) {
                    if profile.draft {
                        self.notice =
                            "Finish this draft before enabling automatic model switching."
                                .to_owned();
                    } else {
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
                        write_settings(&self.settings)?;
                    }
                }
            }
            KeyCode::Char('t') if self.settings_tab == SettingsTab::Privacy => {
                self.set_workspace_trusted(!self.workspace_trusted)?;
            }
            KeyCode::Char('r') if self.settings_tab == SettingsTab::Privacy => {
                self.settings.privacy_acknowledged.clear();
                self.settings.privacy_image_acknowledged.clear();
                write_settings(&self.settings)?;
                self.notice =
                    "Privacy acknowledgements and image-content grants cleared.".to_owned();
            }
            KeyCode::Char('c') if self.settings_tab == SettingsTab::Privacy => {
                provider::save_redaction_values(&[])?;
                self.notice = "Custom local redaction values cleared from the OS credential store."
                    .to_owned();
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn edit_chain(&mut self, index: usize) {
        let Some(chain) = self.settings.model_chains.get(index) else {
            return;
        };
        self.chain_form = Some(ChainDraft {
            original_id: Some(chain.id.clone()),
            alias: chain.alias.clone(),
            id: chain.id.clone(),
            members: chain.members.clone(),
            activate_on_select: chain.activate_on_select,
            focus: 0,
            member_index: 0,
            picking_member: false,
            candidate_index: 0,
        });
    }

    pub(super) fn save_chain(&mut self) -> Result<()> {
        let Some(draft) = self.chain_form.as_ref() else {
            return Ok(());
        };
        let alias = draft.alias.trim().to_owned();
        let id = if draft.id.trim().is_empty() {
            slug(&alias)
        } else {
            slug(&draft.id)
        };
        if alias.is_empty() || id.is_empty() {
            self.notice = "A chain alias and ID are required.".to_owned();
            return Ok(());
        }
        if draft.members.is_empty() {
            self.notice = "Add at least one model to the preference chain.".to_owned();
            return Ok(());
        }
        let candidate_models = available_chain_models(&self.settings);
        if draft.members.iter().any(|member| {
            !candidate_models.iter().any(|(candidate, _)| {
                candidate.provider_id == member.provider_id && candidate.model_id == member.model_id
            })
        }) {
            self.notice = "A chain model refers to a missing or draft provider; edit the chain and remove it.".to_owned();
            return Ok(());
        }
        if self.settings.model_chains.iter().any(|chain| {
            Some(chain.id.as_str()) != draft.original_id.as_deref()
                && chain.id.eq_ignore_ascii_case(&id)
        }) {
            self.notice = format!("A chain with ID `{id}` already exists.");
            return Ok(());
        }
        let chain = ModelChain {
            id: id.clone(),
            alias: alias.clone(),
            members: draft.members.clone(),
            activate_on_select: draft.activate_on_select,
        };
        let old_id = draft.original_id.clone();
        if let Some(index) = self
            .settings
            .model_chains
            .iter()
            .position(|chain| Some(chain.id.as_str()) == draft.original_id.as_deref())
        {
            self.settings.model_chains[index] = chain;
        } else {
            self.settings.model_chains.push(chain);
        }
        if old_id
            .as_deref()
            .is_some_and(|old_id| self.settings.active_chain_id.as_deref() == Some(old_id))
        {
            self.settings.active_chain_id = Some(id.clone());
        }
        self.chain_index = self
            .settings
            .model_chains
            .iter()
            .position(|chain| chain.id == id)
            .unwrap_or(0);
        write_settings(&self.settings)?;
        self.chain_form = None;
        self.notice = format!("Model chain `{alias}` saved with preference order intact.");
        Ok(())
    }

    pub(super) fn handle_chain_form(&mut self, key: event::KeyEvent) -> Result<()> {
        if self
            .chain_form
            .as_ref()
            .is_some_and(|form| form.picking_member)
        {
            let candidates = available_chain_models(&self.settings);
            let form = self.chain_form.as_mut().expect("chain form open");
            match key.code {
                KeyCode::Esc => form.picking_member = false,
                KeyCode::Up | KeyCode::Left => {
                    form.candidate_index = form.candidate_index.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Right => {
                    form.candidate_index =
                        (form.candidate_index + 1).min(candidates.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Some((member, _)) = candidates.get(form.candidate_index)
                        && !form.members.iter().any(|existing| {
                            existing.provider_id == member.provider_id
                                && existing.model_id == member.model_id
                        })
                    {
                        form.members.push(member.clone());
                        form.member_index = form.members.len().saturating_sub(1);
                    }
                    form.picking_member = false;
                }
                _ => {}
            }
            return Ok(());
        }
        let focus = self.chain_form.as_ref().map(|form| form.focus).unwrap_or(0);
        if key.code == KeyCode::Esc {
            self.chain_form = None;
            return Ok(());
        }
        if focus == 3 && key.code == KeyCode::Char('a') {
            let form = self.chain_form.as_mut().expect("chain form open");
            form.picking_member = true;
            form.candidate_index = 0;
            return Ok(());
        }
        if focus == 3
            && !self
                .chain_form
                .as_ref()
                .expect("chain form open")
                .members
                .is_empty()
        {
            match key.code {
                KeyCode::Up => {
                    let form = self.chain_form.as_mut().expect("chain form open");
                    form.member_index = form.member_index.saturating_sub(1);
                    return Ok(());
                }
                KeyCode::Down => {
                    let form = self.chain_form.as_mut().expect("chain form open");
                    form.member_index =
                        (form.member_index + 1).min(form.members.len().saturating_sub(1));
                    return Ok(());
                }
                KeyCode::Left | KeyCode::Right => {
                    let form = self.chain_form.as_mut().expect("chain form open");
                    let index = form.member_index;
                    if key.code == KeyCode::Left && index > 0 {
                        form.members.swap(index, index - 1);
                        form.member_index -= 1;
                    }
                    if key.code == KeyCode::Right && index + 1 < form.members.len() {
                        form.members.swap(index, index + 1);
                        form.member_index += 1;
                    }
                    return Ok(());
                }
                KeyCode::Char('x' | 'X') => {
                    let form = self.chain_form.as_mut().expect("chain form open");
                    if form.member_index < form.members.len() {
                        form.members.remove(form.member_index);
                    }
                    form.member_index = form.member_index.min(form.members.len().saturating_sub(1));
                    return Ok(());
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Tab | KeyCode::Right => {
                self.chain_form.as_mut().expect("chain form open").focus = (focus + 1) % 6
            }
            KeyCode::Up | KeyCode::Left => {
                self.chain_form.as_mut().expect("chain form open").focus = focus.saturating_sub(1)
            }
            KeyCode::Down => {
                self.chain_form.as_mut().expect("chain form open").focus = (focus + 1).min(5)
            }
            KeyCode::Enter if focus == 4 => self.save_chain()?,
            KeyCode::Enter if focus == 5 => self.chain_form = None,
            KeyCode::Enter | KeyCode::Char(' ') if focus == 2 => {
                let form = self.chain_form.as_mut().expect("chain form open");
                form.activate_on_select = !form.activate_on_select;
            }
            KeyCode::Enter => {
                self.chain_form.as_mut().expect("chain form open").focus = (focus + 1).min(5)
            }
            _ => {
                let form = self.chain_form.as_mut().expect("chain form open");
                if focus == 0 {
                    edit_string(&mut form.alias, key);
                }
                if focus == 1 {
                    edit_string(&mut form.id, key);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{remove_provider_profile, unique_provider_alias};
    use crate::tui::draw;
    use crate::tui::state::{App, ModelDraft, ProviderDraft, SettingsTab, adjacent_settings_tab};
    use crate::{ChainModel, ModelChain, ProviderProfile, Settings};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn settings_left_arrow_moves_to_previous_tab() {
        assert_eq!(
            adjacent_settings_tab(SettingsTab::General, false),
            SettingsTab::Privacy
        );
        assert_eq!(
            adjacent_settings_tab(SettingsTab::Privacy, true),
            SettingsTab::General
        );
        assert_eq!(
            adjacent_settings_tab(SettingsTab::AutoSwitch, false),
            SettingsTab::Providers
        );
    }

    #[test]
    fn duplicate_adapter_profiles_get_distinct_aliases() {
        let profile = |id: &str, name: &str| ProviderProfile {
            id: id.to_owned(),
            name: name.to_owned(),
            adapter: "anthropic".to_owned(),
            model: "claude-test".to_owned(),
            models: Vec::new(),
            draft: false,
            auto_switch: false,
            base_url: None,
        };
        let providers = vec![
            profile("one", "Anthropic (Claude)"),
            profile("two", "Anthropic (Claude) 2"),
        ];
        assert_eq!(providers[0].adapter, providers[1].adapter);
        assert_eq!(
            unique_provider_alias(&providers, "Anthropic (Claude)"),
            "Anthropic (Claude) 3"
        );
    }

    #[test]
    fn deleting_provider_cleans_references_and_selects_a_valid_replacement() {
        let profile = |id: &str, model: &str| ProviderProfile {
            id: id.to_owned(),
            name: format!("Provider {id}"),
            adapter: "anthropic".to_owned(),
            model: model.to_owned(),
            models: Vec::new(),
            draft: false,
            auto_switch: false,
            base_url: None,
        };
        let mut settings = Settings::default();
        settings.providers = vec![profile("one", "claude-one"), profile("two", "claude-two")];
        settings.active_provider_id = Some("one".to_owned());
        settings.default_provider_id = Some("one".to_owned());
        settings.provider = Some("anthropic".to_owned());
        settings.model = Some("claude-one".to_owned());
        settings.active_chain_id = Some("chain".to_owned());
        settings.model_chains.push(ModelChain {
            id: "chain".to_owned(),
            alias: "Chain".to_owned(),
            members: vec![
                ChainModel {
                    provider_id: "one".to_owned(),
                    model_id: "claude-one".to_owned(),
                },
                ChainModel {
                    provider_id: "two".to_owned(),
                    model_id: "claude-two".to_owned(),
                },
            ],
            activate_on_select: false,
        });

        assert!(remove_provider_profile(&mut settings, "one"));
        assert_eq!(settings.providers.len(), 1);
        assert_eq!(settings.active_provider_id.as_deref(), Some("two"));
        assert_eq!(settings.default_provider_id.as_deref(), Some("two"));
        assert_eq!(settings.model.as_deref(), Some("claude-two"));
        assert_eq!(settings.model_chains[0].members.len(), 1);
        assert_eq!(settings.model_chains[0].members[0].provider_id, "two");
        assert!(settings.active_chain_id.is_some());
    }

    #[test]
    fn provider_settings_mask_api_key_input() {
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.settings_menu = true;
        app.settings_tab = SettingsTab::Providers;
        app.provider_form = Some(ProviderDraft {
            choosing_preset: false,
            existing_id: None,
            preset: 0,
            alias: "Test Provider".to_owned(),
            base_url: String::new(),
            api_key: "super-secret-value".to_owned(),
            models: vec![ModelDraft {
                id: "test-model".to_owned(),
                name: String::new(),
            }],
            focus: 1,
        });

        terminal
            .draw(|frame| draw(frame, &app, 0))
            .expect("draw settings form");

        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("API Key"));
        assert!(rendered.contains("••••••••••••••••••"));
        assert!(!rendered.contains("super-secret-value"));
    }
}
