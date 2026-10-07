use crate::endpoints::resolve_endpoint;
use crate::tui::models::{available_chain_models, model_name, slug};
use crate::tui::state::{
    App, ChainDraft, ModelDraft, PROVIDER_PRESETS, ProviderDraft, ProviderPreset, edit_string,
};
use crate::{ModelChain, ModelProfile, ProviderProfile, Settings, secrets, write_settings};
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

/// Picks the preset a saved provider was created from, by stable id rather than list position.
pub(super) fn preset_for_profile(profile: &ProviderProfile) -> usize {
    let by_id = |id: &str| {
        PROVIDER_PRESETS
            .iter()
            .position(|preset| preset.id == id)
            .unwrap_or(0)
    };
    if let Some(index) = PROVIDER_PRESETS.iter().position(|preset| {
        preset.base_url.is_some() && preset.base_url == profile.base_url.as_deref()
    }) {
        return index;
    }
    if profile.name.to_ascii_lowercase().contains("openrouter") {
        return by_id("openrouter");
    }
    match profile.adapter.as_str() {
        "openai" => by_id("openai"),
        "anthropic" => by_id("anthropic"),
        "google" => by_id("google"),
        "anthropic-compatible" => by_id("anthropic-custom"),
        _ => by_id("openai-custom"),
    }
}

/// Resolves a provider's models and limits endpoints from the preset's built-in paths or the
/// form's optional fields; both must stay on the provider's own host.
fn provider_endpoints(
    preset: &ProviderPreset,
    draft: &ProviderDraft,
) -> Result<(Option<String>, Option<String>), String> {
    let base = if preset.custom {
        draft.base_url.trim().trim_end_matches('/')
    } else {
        preset.base_url.unwrap_or_default()
    };
    let resolve = |label: &str, typed: &str, built_in: Option<&str>| {
        let input = match (typed.trim(), built_in) {
            ("", None) => return Ok(None),
            ("", Some(path)) => path,
            (typed, _) => typed,
        };
        resolve_endpoint(base, input)
            .map(Some)
            .map_err(|error| format!("{label}: {error}"))
    };
    Ok((
        resolve(
            "Models endpoint",
            &draft.models_endpoint,
            preset.models_path,
        )?,
        resolve(
            "Limits endpoint",
            &draft.limits_endpoint,
            preset.limits_path,
        )?,
    ))
}

/// Custom OpenAI-compatible providers can declare their own models and limits endpoints.
pub(super) fn has_endpoint_fields(form: &ProviderDraft) -> bool {
    PROVIDER_PRESETS[form.preset].id == "openai-custom"
}

pub(super) fn provider_focus_layout(
    form: &ProviderDraft,
) -> (usize, usize, usize, usize, usize, usize) {
    let custom = PROVIDER_PRESETS[form.preset].custom;
    let key_focus = if custom { 2 } else { 1 };
    // Endpoint fields (when shown) sit right after the key: key_focus + 1 and key_focus + 2.
    let model_start = key_focus + if has_endpoint_fields(form) { 3 } else { 1 };
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
        let preset = preset_for_profile(profile);
        // Providers whose model list comes from an endpoint can hold hundreds of models, so the
        // form leaves them to Settings → Models instead of listing every one as an editable row.
        let managed_models = profile.models_url.is_some();
        let models = if managed_models {
            Vec::new()
        } else if profile.models.is_empty() && !profile.model.is_empty() {
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
        let custom_endpoints = PROVIDER_PRESETS[preset].id == "openai-custom";
        self.provider_form = Some(ProviderDraft {
            choosing_preset: false,
            existing_id: Some(profile.id.clone()),
            preset,
            alias: profile.name.clone(),
            suggested_alias: profile.name.clone(),
            base_url: profile.base_url.clone().unwrap_or_default(),
            api_key: String::new(),
            models,
            models_endpoint: custom_endpoints
                .then(|| profile.models_url.clone())
                .flatten()
                .unwrap_or_default(),
            limits_endpoint: custom_endpoints
                .then(|| profile.limits_url.clone())
                .flatten()
                .unwrap_or_default(),
            managed_models,
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
        let name = if draft.alias.trim().is_empty() {
            draft.suggested_alias.trim().to_owned()
        } else {
            draft.alias.trim().to_owned()
        };
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
        let existing_profile = draft
            .existing_id
            .as_deref()
            .and_then(|id| {
                self.settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == id)
            })
            .cloned();
        let models = if draft.managed_models {
            existing_profile
                .as_ref()
                .map(|profile| profile.models.clone())
                .unwrap_or_default()
        } else {
            models
        };
        let (models_url, limits_url) = match provider_endpoints(preset, draft) {
            Ok(urls) => urls,
            Err(message) => {
                self.notice = message;
                return Ok(());
            }
        };
        // A provider that can list its own models may be saved before it has any; it stays a
        // draft until they have been fetched.
        let fetch_after_save = models_url.is_some() && models.is_empty();
        let as_draft = as_draft || fetch_after_save;
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
            models_url,
            limits_url,
            model_info: existing_profile
                .map(|profile| profile.model_info)
                .unwrap_or_default(),
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
        self.notice = if as_draft {
            format!("{name} saved as a draft.")
        } else {
            format!(
                "{name} ({}) saved. Press Enter to make it default or Space to enable automatic switching.",
                preset.label
            )
        };
        if fetch_after_save {
            self.start_models_fetch(self.provider_index, true);
        }
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
                    form.alias = String::new();
                    form.suggested_alias =
                        unique_provider_alias(&self.settings.providers, preset.label);
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
            } else if has_endpoint_fields(form) && focus == key_focus + 1 {
                edit_string(&mut form.models_endpoint, key);
            } else if has_endpoint_fields(form) && focus == key_focus + 2 {
                edit_string(&mut form.limits_endpoint, key);
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
    use crate::tui::render::draw;
    use crate::tui::settings::Section;
    use crate::tui::state::{App, ModelDraft, PROVIDER_PRESETS, ProviderDraft};
    use crate::{ChainModel, ModelChain, ProviderProfile, Settings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
            ..Default::default()
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
            ..Default::default()
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn custom_openai_preset() -> usize {
        PROVIDER_PRESETS
            .iter()
            .position(|preset| preset.label == "Custom OpenAI-compatible API")
            .expect("custom preset")
    }

    fn form_after_choosing_custom_preset() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.open_settings(Section::Providers);
        let mut form = ProviderDraft {
            choosing_preset: true,
            existing_id: None,
            preset: custom_openai_preset(),
            alias: String::new(),
            suggested_alias: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            models: Vec::new(),
            models_endpoint: String::new(),
            limits_endpoint: String::new(),
            managed_models: false,
            focus: 0,
        };
        form.choosing_preset = true;
        app.provider_form = Some(form);
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose preset");
        app
    }

    #[test]
    fn choosing_a_preset_leaves_the_alias_empty_with_a_suggestion() {
        let app = form_after_choosing_custom_preset();
        let form = app.provider_form.as_ref().expect("form");
        assert_eq!(form.alias, "");
        assert_eq!(form.suggested_alias, "Custom OpenAI-compatible API");
    }

    #[test]
    fn typing_an_alias_needs_no_deleting_first() {
        let mut app = form_after_choosing_custom_preset();
        for c in "groq".chars() {
            app.handle_provider_form(key(KeyCode::Char(c)))
                .expect("type");
        }
        assert_eq!(app.provider_form.as_ref().expect("form").alias, "groq");
    }

    #[test]
    fn a_blank_alias_saves_under_the_suggested_name() {
        let mut app = form_after_choosing_custom_preset();
        {
            let form = app.provider_form.as_mut().expect("form");
            form.base_url = "https://example.invalid/v1".to_owned();
        }
        app.save_provider(true).expect("save draft");
        assert_eq!(app.settings.providers.len(), 1);
        assert_eq!(
            app.settings.providers[0].name,
            "Custom OpenAI-compatible API"
        );
    }

    #[test]
    fn the_form_shows_the_suggestion_as_a_placeholder_until_typing() {
        let mut app = form_after_choosing_custom_preset();
        let mut terminal = Terminal::new(TestBackend::new(100, 36)).expect("terminal");
        let text = |terminal: &Terminal<TestBackend>| {
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        assert!(text(&terminal).contains("Custom OpenAI-compatible API"));
        app.handle_provider_form(key(KeyCode::Char('x')))
            .expect("type");
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        assert!(!text(&terminal).contains("Custom OpenAI-compatible API"));
    }

    fn preset_index(id: &str) -> usize {
        PROVIDER_PRESETS
            .iter()
            .position(|preset| preset.id == id)
            .expect("preset")
    }

    fn chosen_form(preset_id: &str) -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.open_settings(Section::Providers);
        app.provider_form = Some(ProviderDraft {
            choosing_preset: true,
            existing_id: None,
            preset: preset_index(preset_id),
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
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose preset");
        app
    }

    #[test]
    fn multiai_is_a_preset_with_built_in_endpoints() {
        let preset = &PROVIDER_PRESETS[preset_index("multiai")];
        assert_eq!(preset.label, "MultiAI");
        assert_eq!(preset.adapter, "openai-compatible");
        assert_eq!(preset.base_url, Some("https://multiai.store/v1"));
        assert!(!preset.custom);
        let base = preset.base_url.unwrap();
        assert_eq!(
            crate::endpoints::resolve_endpoint(base, preset.models_path.unwrap()).unwrap(),
            "https://multiai.store/v1/models"
        );
        assert_eq!(
            crate::endpoints::resolve_endpoint(base, preset.limits_path.unwrap()).unwrap(),
            "https://multiai.store/v1/subscription/limits"
        );
    }

    #[test]
    fn editing_finds_each_provider_s_own_preset_by_id() {
        let profile = |adapter: &str, name: &str, base_url: Option<&str>| ProviderProfile {
            id: "p".to_owned(),
            name: name.to_owned(),
            adapter: adapter.to_owned(),
            base_url: base_url.map(str::to_owned),
            ..Default::default()
        };
        let label =
            |profile: &ProviderProfile| PROVIDER_PRESETS[super::preset_for_profile(profile)].label;
        assert_eq!(
            label(&profile(
                "openai-compatible",
                "x",
                Some("https://multiai.store/v1")
            )),
            "MultiAI"
        );
        assert_eq!(
            label(&profile("openai-compatible", "My OpenRouter", None)),
            "OpenRouter"
        );
        assert_eq!(
            label(&profile(
                "openai-compatible",
                "mine",
                Some("https://x.example/v1")
            )),
            "Custom OpenAI-compatible API"
        );
        assert_eq!(
            label(&profile("anthropic-compatible", "mine", None)),
            "Custom Anthropic-compatible API"
        );
        assert_eq!(
            label(&profile("anthropic", "c", None)),
            "Anthropic (Claude)"
        );
        assert_eq!(label(&profile("google", "g", None)), "Google (Gemini)");
    }

    #[test]
    fn only_custom_openai_forms_have_endpoint_fields_and_they_take_typing() {
        let mut app = chosen_form("openai-custom");
        let (key_focus, model_start, ..) =
            super::provider_focus_layout(app.provider_form.as_ref().unwrap());
        assert_eq!((key_focus, model_start), (2, 5));
        app.provider_form.as_mut().unwrap().focus = 3;
        for c in "models".chars() {
            app.handle_provider_form(key(KeyCode::Char(c)))
                .expect("type");
        }
        app.provider_form.as_mut().unwrap().focus = 4;
        for c in "limits".chars() {
            app.handle_provider_form(key(KeyCode::Char(c)))
                .expect("type");
        }
        let form = app.provider_form.as_ref().unwrap();
        assert_eq!(
            (form.models_endpoint.as_str(), form.limits_endpoint.as_str()),
            ("models", "limits")
        );
        for id in ["multiai", "openai", "anthropic-custom"] {
            let other = chosen_form(id);
            let (_, model_start, ..) =
                super::provider_focus_layout(other.provider_form.as_ref().unwrap());
            let expected = if id == "anthropic-custom" { 3 } else { 2 };
            assert_eq!(model_start, expected, "{id}");
        }
    }

    #[test]
    fn saving_resolves_endpoints_onto_the_profile() {
        let mut app = chosen_form("openai-custom");
        {
            let form = app.provider_form.as_mut().unwrap();
            form.base_url = "https://api.example.com/v1".to_owned();
            form.models_endpoint = "models".to_owned();
            form.limits_endpoint = "https://api.example.com/v1/usage".to_owned();
        }
        app.save_provider(true).expect("save");
        let profile = &app.settings.providers[0];
        assert_eq!(
            profile.models_url.as_deref(),
            Some("https://api.example.com/v1/models")
        );
        assert_eq!(
            profile.limits_url.as_deref(),
            Some("https://api.example.com/v1/usage")
        );
    }

    #[test]
    fn saving_refuses_an_endpoint_on_another_host() {
        let mut app = chosen_form("openai-custom");
        {
            let form = app.provider_form.as_mut().unwrap();
            form.base_url = "https://api.example.com/v1".to_owned();
            form.models_endpoint = "https://evil.example/models".to_owned();
        }
        app.save_provider(true).expect("save");
        assert!(app.settings.providers.is_empty());
        assert!(app.notice.contains("same host"), "{}", app.notice);
        assert!(app.provider_form.is_some());
    }

    #[test]
    fn a_provider_with_a_models_endpoint_and_no_models_saves_as_a_draft() {
        let mut app = chosen_form("multiai");
        app.save_provider(false).expect("save");
        let profile = &app.settings.providers[0];
        assert_eq!(profile.name, "MultiAI");
        assert!(profile.draft);
        assert_eq!(
            profile.models_url.as_deref(),
            Some("https://multiai.store/v1/models")
        );
        assert_eq!(
            profile.limits_url.as_deref(),
            Some("https://multiai.store/v1/subscription/limits")
        );
    }

    #[test]
    fn saving_a_provider_with_a_models_endpoint_starts_loading_its_models() {
        let mut app = chosen_form("multiai");
        app.save_provider(false).expect("save");
        assert_eq!(app.spawned_tasks, 1);
        assert!(app.models_loading.contains(&app.settings.providers[0].id));
        let mut custom = chosen_form("openai-custom");
        custom.provider_form.as_mut().unwrap().base_url = "https://api.example.com/v1".to_owned();
        custom.save_provider(true).expect("save");
        assert_eq!(custom.spawned_tasks, 0);
    }

    #[test]
    fn editing_a_provider_with_a_models_endpoint_keeps_its_models_and_metadata() {
        let mut app = App::new(Settings::default());
        let mut profile = ProviderProfile {
            id: "p1".to_owned(),
            name: "MultiAI".to_owned(),
            adapter: "openai-compatible".to_owned(),
            base_url: Some("https://multiai.store/v1".to_owned()),
            models_url: Some("https://multiai.store/v1/models".to_owned()),
            model: "a".to_owned(),
            models: ["a", "b", "c"]
                .iter()
                .map(|id| crate::ModelProfile {
                    id: (*id).to_owned(),
                    name: String::new(),
                })
                .collect(),
            ..Default::default()
        };
        profile.model_info.insert(
            "a".to_owned(),
            crate::ModelInfo {
                free: Some(true),
                ..Default::default()
            },
        );
        app.settings.providers = vec![profile];
        app.edit_provider(0);
        let form = app.provider_form.as_ref().unwrap();
        assert!(form.managed_models && form.models.is_empty());
        app.save_provider(true).expect("save");
        let saved = &app.settings.providers[0];
        assert_eq!(saved.models.len(), 3);
        assert_eq!(saved.model_info["a"].free, Some(true));
        assert!(saved.models_url.is_some());
    }

    #[test]
    fn the_key_field_says_a_blank_keeps_the_saved_key_when_editing() {
        let mut terminal = Terminal::new(TestBackend::new(110, 40)).expect("terminal");
        let text = |terminal: &Terminal<TestBackend>| {
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        let mut editing = App::new(Settings::default());
        editing.trust_prompt = false;
        editing.open_settings(Section::Providers);
        editing.settings.providers = vec![ProviderProfile {
            id: "p1".to_owned(),
            name: "MultiAI".to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: "m".to_owned(),
            ..Default::default()
        }];
        editing.edit_provider(0);
        terminal
            .draw(|frame| draw(frame, &editing, 0))
            .expect("draw");
        assert!(text(&terminal).contains("leave blank to keep the saved key"));
        let fresh = chosen_form("openai");
        terminal.draw(|frame| draw(frame, &fresh, 0)).expect("draw");
        assert!(!text(&terminal).contains("leave blank to keep"));
    }

    #[test]
    fn editing_an_existing_provider_keeps_its_alias() {
        let mut app = App::new(Settings::default());
        app.settings.providers = vec![ProviderProfile {
            id: "p1".to_owned(),
            name: "My Router".to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: "m".to_owned(),
            models: Vec::new(),
            draft: false,
            auto_switch: false,
            base_url: Some("https://example.invalid/v1".to_owned()),
            ..Default::default()
        }];
        app.edit_provider(0);
        let form = app.provider_form.as_ref().expect("form");
        assert_eq!(form.alias, "My Router");
    }

    #[test]
    fn provider_settings_mask_api_key_input() {
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.open_settings(Section::Providers);
        app.provider_form = Some(ProviderDraft {
            choosing_preset: false,
            existing_id: None,
            preset: 0,
            alias: "Test Provider".to_owned(),
            suggested_alias: String::new(),
            base_url: String::new(),
            api_key: "super-secret-value".to_owned(),
            models: vec![ModelDraft {
                id: "test-model".to_owned(),
                name: String::new(),
            }],
            models_endpoint: String::new(),
            limits_endpoint: String::new(),
            managed_models: false,
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
