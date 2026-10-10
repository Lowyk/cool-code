//! Saving the conversation as a session, and picking an earlier one to continue.

use crate::provider::ChatMessage;
use crate::session::{self, Header, Session, StoredEntry};
use crate::tui::dialog::{Dialog, Routed, Tone, draw_dialog, hint_style, route, window};
use crate::tui::mouse::{Click, Row as MouseRow, line_rect};
use crate::tui::render::centered_rect;
use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::path::PathBuf;

/// How the app was asked to start from an earlier session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resume {
    /// `--resume`: ask which session to continue.
    Pick { all_folders: bool },
    /// `--latest`: continue the most recent one.
    Latest { all_folders: bool },
}

pub(in crate::tui) struct SessionPicker {
    pub(in crate::tui) sessions: Vec<Header>,
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) all_folders: bool,
    pub(in crate::tui) confirm_delete: bool,
}

fn kind_name(kind: TranscriptKind) -> &'static str {
    match kind {
        TranscriptKind::User => "user",
        TranscriptKind::Assistant => "assistant",
        TranscriptKind::CommandOutput => "command",
        TranscriptKind::Error => "error",
    }
}

fn kind_from(name: &str) -> TranscriptKind {
    match name {
        "user" => TranscriptKind::User,
        "assistant" => TranscriptKind::Assistant,
        "error" => TranscriptKind::Error,
        _ => TranscriptKind::CommandOutput,
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn current_folder() -> String {
    std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(in crate::tui) fn default_session_dir() -> PathBuf {
    session::default_dir().unwrap_or_else(|_| std::env::temp_dir().join("coolcode-sessions"))
}

/// A typed prompt, as opposed to a slash command that the transcript also records as "user".
fn is_prompt(entry: &StoredEntry) -> bool {
    entry.kind == "user" && !entry.text.starts_with('/')
}

impl App {
    /// Writes the conversation to disk, if the user opted in and there is something to resume.
    pub(in crate::tui) fn save_session(&mut self) {
        if !self.settings.sessions_enabled || self.messages.is_empty() {
            return;
        }
        let transcript = self
            .transcript
            .iter()
            .map(|entry| StoredEntry {
                kind: kind_name(entry.kind).to_owned(),
                text: entry.text.clone(),
            })
            .collect::<Vec<_>>();
        let provider = self
            .settings
            .active_provider_id
            .as_deref()
            .and_then(|id| {
                self.settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == id)
            })
            .map(|profile| profile.name.clone())
            .unwrap_or_default();
        let saved = Session {
            header: Header {
                id: self.session_id.clone(),
                cwd: current_folder(),
                created: self.session_created,
                updated: now(),
                title: title_of(&transcript),
                provider,
                model: self.settings.model.clone().unwrap_or_default(),
                prompts: transcript.iter().filter(|entry| is_prompt(entry)).count(),
            },
            messages: self.messages.iter().map(Into::into).collect(),
            transcript,
        };
        if let Err(error) = session::save_in(&self.session_dir, &saved) {
            self.notice = format!("Could not save the session: {error:#}");
        }
    }

    /// Gives the next conversation its own session, leaving the old one resumable.
    pub(in crate::tui) fn begin_new_session(&mut self) {
        self.session_created = now();
        self.session_id = session::new_id(self.session_created);
    }

    pub(in crate::tui) fn resume_session(&mut self, id: &str) {
        if self.pending.is_some() {
            self.notice =
                "Wait for the current turn to finish before switching sessions.".to_owned();
            return;
        }
        let loaded = match session::load_in(&self.session_dir, id) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.notice = format!("Could not open that session: {error:#}");
                return;
            }
        };
        // Keep what is on screen safe before it is replaced.
        self.save_session();
        self.messages = loaded.messages.into_iter().map(ChatMessage::from).collect();
        self.transcript = loaded
            .transcript
            .into_iter()
            .map(|entry| TranscriptEntry {
                kind: kind_from(&entry.kind),
                text: entry.text,
            })
            .collect();
        self.session_id = loaded.header.id;
        self.session_created = loaded.header.created;
        self.history_scroll = 0;
        self.streaming = None;
        self.tool_approval = None;
        self.notice = if loaded.header.title.is_empty() {
            "Session resumed.".to_owned()
        } else {
            format!("Resumed: {}", loaded.header.title)
        };
    }

    /// Said when there is nothing to resume and saving is switched off.
    fn saving_hint(&self) -> &'static str {
        if self.settings.sessions_enabled {
            ""
        } else {
            " Saving sessions is off; turn it on in Settings → General."
        }
    }

    fn sessions_here(&self, all_folders: bool) -> Vec<Header> {
        let folder = current_folder();
        session::list_in(&self.session_dir, (!all_folders).then_some(folder.as_str()))
    }

    pub(in crate::tui) fn open_session_picker(&mut self, all_folders: bool) {
        let sessions = self.sessions_here(all_folders);
        if sessions.is_empty() && !all_folders {
            self.notice = if self.sessions_here(true).is_empty() {
                format!("There are no saved sessions yet.{}", self.saving_hint())
            } else {
                "No saved sessions for this folder. Tab in /resume all lists every folder."
                    .to_owned()
            };
            return;
        }
        self.session_picker = Some(SessionPicker {
            sessions,
            selected: 0,
            all_folders,
            confirm_delete: false,
        });
    }

    /// Applies `--resume` / `--latest`.
    pub(crate) fn start_from(&mut self, resume: Resume) {
        match resume {
            Resume::Pick { all_folders } => self.open_session_picker(all_folders),
            Resume::Latest { all_folders } => {
                match self
                    .sessions_here(all_folders)
                    .first()
                    .map(|h| h.id.clone())
                {
                    Some(id) => self.resume_session(&id),
                    None => {
                        self.notice = format!(
                            "No earlier session to continue here; starting a new one.{}",
                            self.saving_hint()
                        );
                    }
                }
            }
        }
    }

    pub(in crate::tui) fn handle_session_picker_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(picker) = self.session_picker.as_mut() else {
            return Ok(());
        };
        if picker.confirm_delete {
            let pressed = match route(&delete_dialog(picker), &mut self.dialog_focus, key) {
                Routed::Press(code) => code,
                Routed::Moved | Routed::Other => return Ok(()),
            };
            picker.confirm_delete = false;
            if pressed == KeyCode::Char('y')
                && let Some(header) = picker.sessions.get(picker.selected).cloned()
            {
                match session::delete_in(&self.session_dir, &header.id) {
                    Ok(()) => {
                        picker.sessions.remove(picker.selected);
                        picker.selected =
                            picker.selected.min(picker.sessions.len().saturating_sub(1));
                        self.notice = format!("Deleted session: {}", header.title);
                    }
                    Err(error) => self.notice = format!("Could not delete it: {error:#}"),
                }
            }
            return Ok(());
        }
        let count = picker.sessions.len();
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(count.saturating_sub(1)),
            KeyCode::Char('x') | KeyCode::Delete if count > 0 => picker.confirm_delete = true,
            KeyCode::Tab => {
                let all_folders = !picker.all_folders;
                self.session_picker = None;
                self.open_session_picker(all_folders);
                if self.session_picker.is_none() {
                    self.session_picker = Some(SessionPicker {
                        sessions: Vec::new(),
                        selected: 0,
                        all_folders,
                        confirm_delete: false,
                    });
                }
            }
            KeyCode::Enter => {
                if let Some(header) = picker.sessions.get(picker.selected).cloned() {
                    self.session_picker = None;
                    self.resume_session(&header.id);
                }
            }
            KeyCode::Esc => self.session_picker = None,
            _ => {}
        }
        Ok(())
    }
}

fn title_of(transcript: &[StoredEntry]) -> String {
    let prompts = transcript
        .iter()
        .filter(|entry| is_prompt(entry))
        .cloned()
        .collect::<Vec<_>>();
    session::title_from(&prompts)
}

/// The question before a saved session is deleted.
fn delete_dialog(picker: &SessionPicker) -> Dialog<'static> {
    let title = picker
        .sessions
        .get(picker.selected)
        .map(|header| header.title.clone())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "(untitled)".to_owned());
    Dialog::confirm(
        "session-delete",
        "Delete session",
        Tone::Danger,
        vec![
            Line::from("Delete this session?"),
            Line::from(Span::styled(title, Style::default().fg(Color::Gray))),
        ],
        "Delete",
        "Cancel",
    )
}

pub(in crate::tui) fn draw_session_picker(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    picker: &SessionPicker,
) {
    let popup = centered_rect(80, 70, area);
    frame.render_widget(Clear, popup);
    let accent = crate::tui::theme::accent_bright();
    let block = window(
        if picker.all_folders {
            "Resume · all folders"
        } else {
            "Resume · this folder"
        },
        Tone::Normal,
    );
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 3 {
        return;
    }
    app.hits.wheel_arrows(popup);
    let now = now();
    let height = usize::from(inner.height.saturating_sub(2));
    let start = (picker.selected + 1).saturating_sub(height);
    let lines = if picker.sessions.is_empty() {
        vec![Line::from(Span::styled(
            "No saved sessions here.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        picker
            .sessions
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(position, header)| {
                let current = position == picker.selected;
                let title = if header.title.is_empty() {
                    "(untitled)"
                } else {
                    header.title.as_str()
                };
                let mut detail = format!(
                    "{} · {} prompt{}",
                    session::ago(now, header.updated),
                    header.prompts,
                    if header.prompts == 1 { "" } else { "s" }
                );
                if !header.model.is_empty() {
                    detail.push_str(&format!(" · {}", header.model));
                }
                if picker.all_folders {
                    let folder = std::path::Path::new(&header.cwd)
                        .file_name()
                        .map_or(header.cwd.clone(), |name| {
                            name.to_string_lossy().into_owned()
                        });
                    detail.push_str(&format!(" · {folder}"));
                }
                Line::from(vec![
                    Span::styled(
                        if current { "› " } else { "  " },
                        Style::default().fg(accent),
                    ),
                    Span::styled(
                        title.to_owned(),
                        if current {
                            Style::default().fg(accent).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                    Span::styled(format!("   {detail}"), Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect()
    };
    let list = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    frame.render_widget(Paragraph::new(lines), list);
    for (row, position) in (start..picker.sessions.len().min(start + height)).enumerate() {
        app.hits.click(
            line_rect(list, row),
            Click::Row(MouseRow::new(position, picker.selected)),
        );
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            "↑↓ move   Enter resume   x delete   Tab all folders   Esc close",
            hint_style(),
        ))
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
    if picker.confirm_delete {
        draw_dialog(
            frame,
            area,
            &delete_dialog(picker),
            &app.dialog_focus,
            &app.hits,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Resume, kind_from, kind_name};
    use crate::Settings;
    use crate::provider::ChatMessage;
    use crate::session::{self, Header, Session, StoredEntry};
    use crate::tui::render::draw;
    use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.settings.sessions_enabled = true;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn talk(app: &mut App, prompt: &str, answer: &str) {
        app.messages.push(ChatMessage::user_with_images(
            prompt.to_owned(),
            prompt.to_owned(),
            Vec::new(),
        ));
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: prompt.to_owned(),
        });
        app.messages.push(ChatMessage::assistant(answer.to_owned()));
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Assistant,
            text: answer.to_owned(),
        });
    }

    fn stored(id: &str, cwd: &str, updated: i64, prompt: &str) -> Session {
        Session {
            header: Header {
                id: id.to_owned(),
                cwd: cwd.to_owned(),
                created: updated - 5,
                updated,
                title: prompt.to_owned(),
                provider: String::new(),
                model: "m".to_owned(),
                prompts: 1,
            },
            messages: vec![(&ChatMessage::assistant(format!("re: {prompt}"))).into()],
            transcript: vec![
                StoredEntry {
                    kind: "user".to_owned(),
                    text: prompt.to_owned(),
                },
                StoredEntry {
                    kind: "assistant".to_owned(),
                    text: format!("re: {prompt}"),
                },
            ],
        }
    }

    fn here() -> String {
        std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn put(app: &App, session: &Session) {
        session::save_in(&app.session_dir, session).expect("save");
    }

    #[test]
    fn nothing_is_saved_until_the_user_opts_in() {
        let mut app = app();
        app.settings.sessions_enabled = false;
        talk(&mut app, "private thoughts", "ok");
        app.save_session();
        assert!(session::list_in(&app.session_dir, None).is_empty());
        assert!(!app.session_dir.exists(), "not even the folder is created");
        app.input = "/clear".to_owned();
        app.submit().expect("clear");
        app.input = "/quit".to_owned();
        app.submit().expect("quit");
        assert!(!app.session_dir.exists());
    }

    #[test]
    fn turning_saving_on_starts_saving_from_then_on() {
        let mut app = app();
        app.settings.sessions_enabled = false;
        talk(&mut app, "before", "ok");
        app.save_session();
        app.settings.sessions_enabled = true;
        talk(&mut app, "after", "ok");
        app.save_session();
        let listed = session::list_in(&app.session_dir, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].prompts, 2,
            "the whole conversation is saved once enabled"
        );
    }

    #[test]
    fn old_sessions_stay_resumable_even_with_saving_off() {
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "from before"));
        app.settings.sessions_enabled = false;
        app.start_from(Resume::Latest { all_folders: false });
        assert_eq!(app.session_id, "s1-aaaa");
    }

    #[test]
    fn with_saving_off_an_empty_resume_says_how_to_turn_it_on() {
        let mut app = app();
        app.settings.sessions_enabled = false;
        app.start_from(Resume::Pick { all_folders: false });
        assert!(app.notice.contains("Settings → General"), "{}", app.notice);
        app.settings.sessions_enabled = true;
        app.start_from(Resume::Pick { all_folders: false });
        assert!(!app.notice.contains("Settings"), "{}", app.notice);
    }

    #[test]
    fn an_empty_conversation_is_not_saved() {
        let mut app = app();
        app.save_session();
        assert!(session::list_in(&app.session_dir, None).is_empty());
    }

    #[test]
    fn a_conversation_is_saved_with_a_title_and_prompt_count() {
        let mut app = app();
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: "/model fable".to_owned(),
        });
        talk(&mut app, "refactor the parser", "done");
        app.save_session();
        let listed = session::list_in(&app.session_dir, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "refactor the parser");
        assert_eq!(listed[0].prompts, 1, "slash commands are not prompts");
        assert_eq!(listed[0].cwd, here());
    }

    #[test]
    fn saving_twice_keeps_one_session_that_grows() {
        let mut app = app();
        talk(&mut app, "first", "a");
        app.save_session();
        talk(&mut app, "second", "b");
        app.save_session();
        let listed = session::list_in(&app.session_dir, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].prompts, 2);
    }

    #[test]
    fn resuming_restores_the_conversation_and_keeps_appending_to_it() {
        let mut app = app();
        talk(&mut app, "build a parser", "ok");
        app.save_session();
        let id = app.session_id.clone();

        let mut next = app_sharing(&app);
        next.resume_session(&id);
        assert_eq!(next.messages.len(), 2);
        assert_eq!(next.transcript.len(), 2);
        assert_eq!(next.transcript[1].kind, TranscriptKind::Assistant);
        assert_eq!(next.session_id, id);
        assert!(next.notice.contains("build a parser"), "{}", next.notice);
        talk(&mut next, "more", "sure");
        next.save_session();
        assert_eq!(session::list_in(&next.session_dir, None).len(), 1);
        assert_eq!(
            session::load_in(&next.session_dir, &id)
                .unwrap()
                .messages
                .len(),
            4
        );
    }

    /// A second app that looks at the same saved sessions.
    fn app_sharing(other: &App) -> App {
        let mut app = app();
        app.session_dir = other.session_dir.clone();
        app
    }

    #[test]
    fn a_missing_session_is_reported_not_fatal() {
        let mut app = app();
        talk(&mut app, "keep me", "ok");
        app.resume_session("s0-nope");
        assert!(app.notice.contains("Could not open"), "{}", app.notice);
        assert_eq!(
            app.messages.len(),
            2,
            "the current conversation is untouched"
        );
    }

    #[test]
    fn clearing_starts_a_new_session_and_keeps_the_old_one() {
        let mut app = app();
        talk(&mut app, "old chat", "ok");
        app.input = "/clear".to_owned();
        app.submit().expect("clear");
        assert!(app.messages.is_empty());
        talk(&mut app, "new chat", "ok");
        app.save_session();
        let titles = session::list_in(&app.session_dir, None)
            .into_iter()
            .map(|header| header.title)
            .collect::<Vec<_>>();
        assert_eq!(titles.len(), 2, "{titles:?}");
        assert!(titles.contains(&"old chat".to_owned()));
        assert!(titles.contains(&"new chat".to_owned()));
    }

    #[test]
    fn latest_continues_the_newest_session_of_this_folder() {
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "older"));
        put(&app, &stored("s2-bbbb", &here(), 200, "newer"));
        put(&app, &stored("s3-cccc", "/elsewhere", 300, "other folder"));
        app.start_from(Resume::Latest { all_folders: false });
        assert_eq!(app.session_id, "s2-bbbb");
        assert!(app.notice.contains("newer"), "{}", app.notice);

        let mut everywhere = app_sharing(&app);
        everywhere.start_from(Resume::Latest { all_folders: true });
        assert_eq!(everywhere.session_id, "s3-cccc");
    }

    #[test]
    fn latest_with_nothing_saved_says_so_and_starts_fresh() {
        let mut app = app();
        let id = app.session_id.clone();
        app.start_from(Resume::Latest { all_folders: false });
        assert_eq!(app.session_id, id);
        assert!(app.notice.contains("No earlier session"), "{}", app.notice);
    }

    #[test]
    fn the_picker_lists_this_folder_and_enter_resumes() {
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "older"));
        put(&app, &stored("s2-bbbb", &here(), 200, "newer"));
        put(&app, &stored("s3-cccc", "/elsewhere", 300, "other folder"));
        app.start_from(Resume::Pick { all_folders: false });
        let picker = app.session_picker.as_ref().expect("picker");
        assert_eq!(picker.sessions.len(), 2);
        app.handle_session_picker_key(key(KeyCode::Down)).unwrap();
        app.handle_session_picker_key(key(KeyCode::Enter)).unwrap();
        assert!(app.session_picker.is_none());
        assert_eq!(app.session_id, "s1-aaaa");
        assert_eq!(app.transcript[0].text, "older");
    }

    #[test]
    fn tab_widens_the_picker_to_every_folder() {
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "here"));
        put(&app, &stored("s3-cccc", "/elsewhere", 300, "there"));
        app.start_from(Resume::Pick { all_folders: false });
        assert_eq!(app.session_picker.as_ref().unwrap().sessions.len(), 1);
        app.handle_session_picker_key(key(KeyCode::Tab)).unwrap();
        let picker = app.session_picker.as_ref().unwrap();
        assert!(picker.all_folders);
        assert_eq!(picker.sessions.len(), 2);
        app.handle_session_picker_key(key(KeyCode::Tab)).unwrap();
        assert_eq!(app.session_picker.as_ref().unwrap().sessions.len(), 1);
    }

    #[test]
    fn deleting_a_session_needs_confirmation() {
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "doomed"));
        put(&app, &stored("s2-bbbb", &here(), 200, "kept"));
        app.start_from(Resume::Pick { all_folders: false });
        app.handle_session_picker_key(key(KeyCode::Char('x')))
            .unwrap();
        app.handle_session_picker_key(key(KeyCode::Char('n')))
            .unwrap();
        assert_eq!(session::list_in(&app.session_dir, None).len(), 2);
        app.handle_session_picker_key(key(KeyCode::Down)).unwrap();
        app.handle_session_picker_key(key(KeyCode::Char('x')))
            .unwrap();
        app.handle_session_picker_key(key(KeyCode::Char('y')))
            .unwrap();
        let left = session::list_in(&app.session_dir, None);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].title, "kept");
        assert_eq!(app.session_picker.as_ref().unwrap().sessions.len(), 1);
    }

    #[test]
    fn clicking_a_session_selects_it_and_a_second_click_resumes_it() {
        use crate::tui::mouse::testing::click_text;
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "older talk"));
        put(&app, &stored("s2-bbbb", &here(), 200, "newer talk"));
        app.start_from(Resume::Pick { all_folders: false });
        click_text(&mut app, "older talk");
        assert_eq!(app.session_picker.as_ref().map(|p| p.selected), Some(1));
        click_text(&mut app, "older talk");
        assert!(app.session_picker.is_none());
        assert_eq!(app.session_id, "s1-aaaa");
    }

    #[test]
    fn deleting_a_session_asks_in_a_shared_dialog_where_enter_cancels() {
        use crate::tui::mouse::testing::{click_text, has_button};
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "doomed"));
        put(&app, &stored("s2-bbbb", &here(), 200, "kept"));
        app.start_from(Resume::Pick { all_folders: false });
        app.handle_session_picker_key(key(KeyCode::Down)).unwrap();
        app.handle_session_picker_key(key(KeyCode::Char('x')))
            .unwrap();
        assert!(has_button(&app, "Delete"));
        app.handle_session_picker_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(session::list_in(&app.session_dir, None).len(), 2);
        assert!(app.session_picker.is_some(), "the picker stays open");
        app.handle_session_picker_key(key(KeyCode::Char('x')))
            .unwrap();
        click_text(&mut app, "[ Delete (y)");
        let left = session::list_in(&app.session_dir, None);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].title, "kept");
    }

    #[test]
    fn esc_closes_the_picker_without_changing_anything() {
        let mut app = app();
        put(&app, &stored("s1-aaaa", &here(), 100, "x"));
        app.start_from(Resume::Pick { all_folders: false });
        let id = app.session_id.clone();
        app.handle_session_picker_key(key(KeyCode::Esc)).unwrap();
        assert!(app.session_picker.is_none());
        assert_eq!(app.session_id, id);
    }

    #[test]
    fn the_picker_refuses_to_open_when_there_is_nothing_to_pick() {
        let mut app = app();
        app.start_from(Resume::Pick { all_folders: false });
        assert!(app.session_picker.is_none());
        assert!(app.notice.contains("no saved sessions"), "{}", app.notice);
        put(&app, &stored("s3-cccc", "/elsewhere", 300, "there"));
        app.start_from(Resume::Pick { all_folders: false });
        assert!(app.session_picker.is_none());
        assert!(app.notice.contains("this folder"), "{}", app.notice);
    }

    #[test]
    fn the_picker_draws_titles_and_ages() {
        let mut app = app();
        put(
            &app,
            &stored("s1-aaaa", &here(), 100, "refactor the parser"),
        );
        app.start_from(Resume::Pick { all_folders: false });
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("terminal");
        terminal.draw(|frame| draw(frame, &app, 0)).expect("draw");
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("refactor the parser"), "{screen}");
        assert!(screen.contains("1 prompt"), "{screen}");
        assert!(screen.contains("Resume"), "{screen}");
    }

    #[test]
    fn transcript_kinds_survive_their_names() {
        for kind in [
            TranscriptKind::User,
            TranscriptKind::Assistant,
            TranscriptKind::CommandOutput,
            TranscriptKind::Error,
        ] {
            assert_eq!(kind_from(kind_name(kind)), kind);
        }
    }
}
