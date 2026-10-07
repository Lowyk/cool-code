//! Signing in to a ChatGPT Plus/Pro account instead of using an API key.
//!
//! This is unofficial: it uses the same browser sign-in as OpenAI's Codex CLI, is not a
//! supported API, may stop working without notice, and may conflict with OpenAI's terms. The
//! screens here say so before anything happens.

use crate::chatgpt_auth::{self, Account, LoginFlow, Session};
use crate::tui::forms::unique_provider_alias;
use crate::tui::render::centered_rect;
use crate::tui::settings::sync::TaskResult;
use crate::tui::state::App;
use crate::{ProviderProfile, write_settings};
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The name a ChatGPT sign-in is saved under.
const PROFILE_NAME: &str = "ChatGPT Plus/Pro (unofficial)";

/// The ways to use OpenAI, in the order they are listed.
const METHODS: [(&str, &str); 2] = [
    ("API key", "pay per use · the official way"),
    (
        "ChatGPT Plus/Pro",
        "sign in with your subscription · unofficial",
    ),
];

enum Stage {
    /// Asking how to sign in.
    Choosing(usize),
    /// The browser is open and the local server is waiting for it to come back.
    Waiting {
        url: String,
        cancel: Arc<AtomicBool>,
        browser_opened: bool,
    },
}

pub(in crate::tui) struct ChatGptLogin {
    stage: Stage,
    /// The provider being signed in to again, when this is not a new connection.
    reauth: Option<String>,
}

#[cfg(not(test))]
fn new_flow() -> anyhow::Result<LoginFlow> {
    LoginFlow::start()
}

// Tests must not take over the real callback port.
#[cfg(test)]
fn new_flow() -> anyhow::Result<LoginFlow> {
    LoginFlow::start_on("127.0.0.1:0")
}

#[cfg(not(test))]
fn open_browser(url: &str) -> bool {
    chatgpt_auth::open_browser(url)
}

#[cfg(test)]
fn open_browser(_url: &str) -> bool {
    false
}

impl App {
    /// Offers the two ways to use OpenAI: an API key, or a ChatGPT subscription.
    pub(in crate::tui) fn ask_openai_sign_in_method(&mut self) {
        self.chatgpt_login = Some(ChatGptLogin {
            stage: Stage::Choosing(0),
            reauth: None,
        });
    }

    /// Opens the browser and waits for the sign-in to finish in the background.
    pub(in crate::tui) fn start_chatgpt_login(&mut self, reauth: Option<String>) {
        let flow = match new_flow() {
            Ok(flow) => flow,
            Err(error) => {
                self.chatgpt_login = None;
                self.notice = format!("{error:#}");
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let url = flow.url.clone();
        let browser_opened = open_browser(&url);
        let watch = cancel.clone();
        self.spawn_task(move || TaskResult::Login {
            result: flow
                .finish(chatgpt_auth::TOKEN_URL, &watch)
                .map_err(|error| format!("{error:#}")),
        });
        self.chatgpt_login = Some(ChatGptLogin {
            stage: Stage::Waiting {
                url,
                cancel,
                browser_opened,
            },
            reauth,
        });
    }

    pub(in crate::tui) fn handle_chatgpt_login_key(&mut self, key: event::KeyEvent) {
        let Some(login) = self.chatgpt_login.as_mut() else {
            return;
        };
        match &mut login.stage {
            Stage::Choosing(selected) => match key.code {
                KeyCode::Up | KeyCode::Left => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Right => {
                    *selected = (*selected + 1).min(METHODS.len() - 1)
                }
                KeyCode::Esc => self.chatgpt_login = None,
                KeyCode::Enter => {
                    let chosen = *selected;
                    self.chatgpt_login = None;
                    if chosen == 0 {
                        self.apply_preset_choice();
                    } else {
                        self.start_chatgpt_login(None);
                    }
                }
                _ => {}
            },
            Stage::Waiting { cancel, .. } => {
                if key.code == KeyCode::Esc {
                    cancel.store(true, Ordering::Relaxed);
                    self.chatgpt_login = None;
                    self.notice = "ChatGPT sign-in cancelled.".to_owned();
                }
            }
        }
    }

    /// Saves a completed sign-in as a provider (or renews the one it was for).
    pub(in crate::tui) fn finish_chatgpt_login(
        &mut self,
        result: Result<(Account, Session), String>,
    ) {
        // A sign-in that was cancelled in the meantime has nothing waiting for it.
        let Some(login) = self
            .chatgpt_login
            .take_if(|login| matches!(login.stage, Stage::Waiting { .. }))
        else {
            return;
        };
        let (account, session) = match result {
            Ok(signed_in) => signed_in,
            Err(error) => {
                self.notice = format!("ChatGPT sign-in failed: {error}");
                return;
            }
        };
        let existing = login.reauth.filter(|id| {
            self.settings
                .providers
                .iter()
                .any(|profile| &profile.id == id)
        });
        let id = existing
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if let Err(error) = chatgpt_auth::remember_login(&id, &account, session) {
            self.notice = format!("ChatGPT sign-in worked but could not be saved: {error:#}");
            return;
        }
        let who = account
            .email
            .clone()
            .unwrap_or_else(|| "your ChatGPT account".to_owned());
        if existing.is_some() {
            if let Some(index) = self
                .settings
                .providers
                .iter()
                .position(|profile| profile.id == id)
            {
                self.start_models_fetch(index, false);
            }
            self.notice = format!("Signed in to ChatGPT again as {who}.");
            return;
        }
        let profile = ProviderProfile {
            id: id.clone(),
            name: unique_provider_alias(&self.settings.providers, PROFILE_NAME),
            adapter: "chatgpt".to_owned(),
            // Models come from the account itself, right after signing in.
            base_url: Some(chatgpt_auth::API_BASE.to_owned()),
            models_url: Some(chatgpt_auth::MODELS_URL.to_owned()),
            limits_url: Some(chatgpt_auth::USAGE_URL.to_owned()),
            ..Default::default()
        };
        let previous = self.settings.clone();
        self.settings.providers.push(profile.clone());
        if self.settings.default_provider_id.is_none() {
            self.settings.default_provider_id = Some(id.clone());
        }
        if self.settings.active_provider_id.is_none() {
            self.settings.active_provider_id = Some(id.clone());
            self.settings.provider = Some(profile.adapter.clone());
            self.settings.model = None;
            self.settings.base_url = profile.base_url.clone();
            self.settings.api_key_env = None;
        }
        if let Err(error) = write_settings(&self.settings) {
            self.settings = previous;
            let _ = chatgpt_auth::sign_out(&id);
            self.notice = format!("Signed in, but settings could not be saved: {error:#}");
            return;
        }
        self.provider_form = None;
        self.provider_index = self.settings.providers.len() - 1;
        self.start_models_fetch(self.provider_index, false);
        self.start_limits_fetch(self.provider_index, true);
        self.notice =
            format!("Signed in to ChatGPT as {who}. It is unofficial and may stop working.");
    }
}

pub(in crate::tui) fn draw_chatgpt_login(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    login: &ChatGptLogin,
) {
    let popup = centered_rect(72, 62, area);
    frame.render_widget(Clear, popup);
    let accent = crate::tui::theme::accent_bright();
    let title = match login.stage {
        Stage::Choosing(_) => " OpenAI · how do you want to sign in? ",
        Stage::Waiting { .. } => " ChatGPT sign-in (unofficial) ",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(crate::tui::theme::panel()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let dim = Style::default().fg(Color::DarkGray);
    let warning = Style::default().fg(Color::Rgb(240, 210, 90));
    let mut lines = Vec::new();
    match &login.stage {
        Stage::Choosing(selected) => {
            for (index, (name, detail)) in METHODS.iter().enumerate() {
                let current = index == *selected;
                lines.push(Line::from(vec![
                    Span::styled(
                        if current { "› " } else { "  " },
                        Style::default().fg(accent),
                    ),
                    Span::styled(
                        *name,
                        Style::default()
                            .fg(if current { Color::White } else { Color::Gray })
                            .add_modifier(if current {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                    Span::styled(format!("  {detail}"), dim),
                ]));
            }
            lines.push(Line::from(""));
            if *selected == 1 {
                lines.push(Line::from(Span::styled(
                    "Unofficial. This uses the same sign-in as OpenAI's Codex CLI. It is not a supported API, may stop working without notice, and may conflict with OpenAI's terms of use. Only a renewable sign-in token is kept, in your OS credential store.",
                    warning,
                )));
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "↑/↓ choose · Enter continue · Esc back",
                dim,
            )));
        }
        Stage::Waiting {
            url,
            browser_opened,
            ..
        } => {
            lines.push(Line::from(if *browser_opened {
                "Finish signing in in your browser. Waiting…"
            } else {
                "Open this address in a browser to sign in. Waiting…"
            }));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                url.clone(),
                Style::default().fg(accent),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "The page returns to this program on localhost:1455. Esc cancels.",
                dim,
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;
    use crate::tui::settings::Section;
    use crate::tui::state::PROVIDER_PRESETS;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_choosing(preset_id: &str) -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.open_settings(Section::Providers);
        app.provider_form = Some(crate::tui::state::ProviderDraft {
            choosing_preset: true,
            existing_id: None,
            preset: PROVIDER_PRESETS
                .iter()
                .position(|preset| preset.id == preset_id)
                .expect("preset"),
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
        app
    }

    fn account(email: &str) -> Account {
        Account {
            refresh_token: "refresh-1".to_owned(),
            account_id: Some("acct-1".to_owned()),
            email: Some(email.to_owned()),
        }
    }

    fn session() -> Session {
        Session {
            access_token: "access-1".to_owned(),
            expires_at: chrono::Utc::now().timestamp() + 3600,
            account_id: Some("acct-1".to_owned()),
        }
    }

    fn waiting_app() -> App {
        let mut app = app_choosing("openai");
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose");
        app.handle_chatgpt_login_key(key(KeyCode::Down));
        app.handle_chatgpt_login_key(key(KeyCode::Enter));
        app
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
    fn choosing_openai_asks_how_to_sign_in_before_the_key_form() {
        let mut app = app_choosing("openai");
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose");
        assert!(app.chatgpt_login.is_some());
        assert!(app.provider_form.as_ref().unwrap().choosing_preset);
        let shown = screen(&app);
        assert!(
            shown.contains("API key") && shown.contains("ChatGPT Plus/Pro"),
            "{shown}"
        );
    }

    #[test]
    fn other_presets_do_not_ask() {
        let mut app = app_choosing("openrouter");
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose");
        assert!(app.chatgpt_login.is_none());
        assert!(!app.provider_form.as_ref().unwrap().choosing_preset);
    }

    #[test]
    fn the_api_key_answer_continues_to_the_key_form() {
        let mut app = app_choosing("openai");
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose");
        app.handle_chatgpt_login_key(key(KeyCode::Enter));
        assert!(app.chatgpt_login.is_none());
        let form = app.provider_form.as_ref().unwrap();
        assert!(!form.choosing_preset);
        assert_eq!(form.suggested_alias, "OpenAI (ChatGPT)");
    }

    #[test]
    fn escape_goes_back_to_the_preset_list() {
        let mut app = app_choosing("openai");
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose");
        app.handle_chatgpt_login_key(key(KeyCode::Esc));
        assert!(app.chatgpt_login.is_none());
        assert!(app.provider_form.as_ref().unwrap().choosing_preset);
    }

    #[test]
    fn the_subscription_answer_warns_that_it_is_unofficial() {
        let mut app = app_choosing("openai");
        app.handle_provider_form(key(KeyCode::Enter))
            .expect("choose");
        assert!(
            !screen(&app).contains("Unofficial."),
            "only shown for the subscription"
        );
        app.handle_chatgpt_login_key(key(KeyCode::Down));
        let shown = screen(&app);
        assert!(shown.contains("Unofficial."), "{shown}");
        assert!(shown.contains("terms of use"), "{shown}");
    }

    #[test]
    fn the_subscription_answer_waits_for_the_browser_and_shows_the_address() {
        let app = waiting_app();
        assert_eq!(app.spawned_tasks, 1);
        let shown = screen(&app);
        assert!(shown.contains("auth.openai.com/oauth/authorize"), "{shown}");
        assert!(shown.contains("Esc cancels"), "{shown}");
    }

    #[test]
    fn escape_while_waiting_cancels_and_a_late_result_is_ignored() {
        let mut app = waiting_app();
        let cancel = match &app.chatgpt_login.as_ref().unwrap().stage {
            Stage::Waiting { cancel, .. } => cancel.clone(),
            Stage::Choosing(_) => panic!("should be waiting"),
        };
        app.handle_chatgpt_login_key(key(KeyCode::Esc));
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.chatgpt_login.is_none());
        app.apply_task_result(TaskResult::Login {
            result: Ok((account("late@example.com"), session())),
        });
        assert!(app.settings.providers.is_empty());
    }

    #[test]
    fn a_finished_sign_in_becomes_an_unofficial_provider_and_the_default() {
        let mut app = waiting_app();
        app.apply_task_result(TaskResult::Login {
            result: Ok((account("me@example.com"), session())),
        });
        assert!(app.chatgpt_login.is_none() && app.provider_form.is_none());
        let profile = &app.settings.providers[0];
        assert_eq!(profile.name, "ChatGPT Plus/Pro (unofficial)");
        assert_eq!(profile.adapter, "chatgpt");
        assert_eq!(profile.base_url.as_deref(), Some(chatgpt_auth::API_BASE));
        assert!(
            !profile.draft && profile.models.is_empty(),
            "models arrive from the account"
        );
        assert_eq!(
            app.settings.default_provider_id.as_deref(),
            Some(profile.id.as_str())
        );
        assert_eq!(
            app.settings.active_provider_id.as_deref(),
            Some(profile.id.as_str())
        );
        assert_eq!(app.settings.provider.as_deref(), Some("chatgpt"));
        assert!(app.notice.contains("me@example.com"), "{}", app.notice);
        let saved = chatgpt_auth::load_account(&profile.id)
            .unwrap()
            .expect("account saved");
        assert_eq!(saved.refresh_token, "refresh-1");
    }

    #[test]
    fn a_failed_sign_in_adds_nothing_and_says_why() {
        let mut app = waiting_app();
        app.apply_task_result(TaskResult::Login {
            result: Err("timed out waiting for the browser sign-in to finish".to_owned()),
        });
        assert!(app.settings.providers.is_empty());
        assert!(app.chatgpt_login.is_none());
        assert!(
            app.notice.contains("failed") && app.notice.contains("timed out"),
            "{}",
            app.notice
        );
    }

    #[test]
    fn signing_in_again_renews_the_same_provider() {
        let mut app = waiting_app();
        app.apply_task_result(TaskResult::Login {
            result: Ok((account("me@example.com"), session())),
        });
        app.edit_provider(0);
        assert!(app.provider_form.is_none(), "there is no form to edit");
        assert!(matches!(
            app.chatgpt_login.as_ref().map(|login| &login.stage),
            Some(Stage::Waiting { .. })
        ));
        let id = app.settings.providers[0].id.clone();
        app.apply_task_result(TaskResult::Login {
            result: Ok((
                Account {
                    refresh_token: "refresh-2".to_owned(),
                    ..account("me@example.com")
                },
                session(),
            )),
        });
        assert_eq!(app.settings.providers.len(), 1);
        assert_eq!(
            chatgpt_auth::load_account(&id)
                .unwrap()
                .unwrap()
                .refresh_token,
            "refresh-2"
        );
        assert!(app.notice.contains("again"), "{}", app.notice);
    }

    #[test]
    fn a_new_sign_in_loads_the_accounts_current_models() {
        let mut app = waiting_app();
        let before = app.spawned_tasks;
        app.apply_task_result(TaskResult::Login {
            result: Ok((account("me@example.com"), session())),
        });
        assert_eq!(app.spawned_tasks, before + 2, "models and usage");
        let profile = &app.settings.providers[0];
        let ids: Vec<_> = profile
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert!(ids.is_empty(), "no model names are built in: {ids:?}");
        assert_eq!(app.settings.model, None);
        assert_eq!(
            profile.models_url.as_deref(),
            Some("https://chatgpt.com/backend-api/codex/models")
        );
        assert!(app.models_loading.contains(&profile.id));
        assert_eq!(
            profile.limits_url.as_deref(),
            Some("https://chatgpt.com/backend-api/wham/usage")
        );
        assert!(app.limits.contains_key(&profile.id), "usage is loading");
        assert!(app.notice.contains("me@example.com"), "{}", app.notice);
    }

    #[test]
    fn the_accounts_own_list_replaces_retired_models() {
        use crate::endpoints::FetchedModel;
        let mut app = waiting_app();
        app.apply_task_result(TaskResult::Login {
            result: Ok((account("me@example.com"), session())),
        });
        let id = app.settings.providers[0].id.clone();
        app.settings.providers[0].models.push(crate::ModelProfile {
            id: "retired-model".to_owned(),
            name: String::new(),
        });
        app.settings.providers[0].model = "retired-model".to_owned();
        let fetched = |id: &str| FetchedModel {
            id: id.to_owned(),
            name: String::new(),
            free: None,
            tools: None,
            context: None,
        };
        app.apply_task_result(TaskResult::Models {
            provider_id: id,
            url: "https://chatgpt.com/backend-api/codex/models".to_owned(),
            promote: false,
            result: Ok(vec![fetched("gpt-6.1"), fetched("gpt-6")]),
        });
        let profile = &app.settings.providers[0];
        let ids: Vec<_> = profile
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(ids, ["gpt-6.1", "gpt-6"]);
        assert_eq!(
            profile.model, "gpt-6.1",
            "a retired default moves to the first listed model"
        );
        assert_eq!(app.settings.model.as_deref(), Some("gpt-6.1"));
        assert_eq!(
            app.notice, "ChatGPT Plus/Pro (unofficial): 2 models loaded from your account.",
            "the count is the real number of models"
        );
    }

    #[test]
    fn deleting_the_provider_forgets_the_sign_in() {
        let mut app = waiting_app();
        app.apply_task_result(TaskResult::Login {
            result: Ok((account("me@example.com"), session())),
        });
        let id = app.settings.providers[0].id.clone();
        app.delete_provider(0).expect("delete");
        assert!(app.settings.providers.is_empty());
        assert!(chatgpt_auth::load_account(&id).unwrap().is_none());
    }

    #[test]
    fn the_provider_details_describe_the_sign_in_instead_of_a_key() {
        let mut app = waiting_app();
        app.apply_task_result(TaskResult::Login {
            result: Ok((account("me@example.com"), session())),
        });
        app.open_settings(Section::Providers);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus content");
        let shown = screen(&app);
        assert!(shown.contains("ChatGPT account (unofficial)"), "{shown}");
        assert!(!shown.contains("OS credential store ·"), "{shown}");
    }
}
