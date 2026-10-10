//! The subagent tracker: Shift+↓ opens a window listing every subagent of the current turn with
//! its task, status, progress and latest action, updated as the worker reports. x cancels the
//! highlighted subagent only; Esc or Shift+↑ closes the window.

use crate::tui::dialog::{Button, Dialog, Routed, Tone, draw_buttons, route, text_rows, window};
use crate::tui::mouse::{Click, Row as MouseRow, line_rect};
use crate::tui::state::App;
use crate::workflow::{SubagentEvent, SubagentOutcome};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph, Wrap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Rows of the highlighted subagent's task shown under the list.
const TASK_ROWS: u16 = 4;
/// Columns of the status and the label in each row.
const STATUS_WIDTH: usize = 11;
const LABEL_WIDTH: usize = 30;
const PROGRESS_WIDTH: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

pub(in crate::tui) struct Tracked {
    pub(in crate::tui) id: usize,
    pub(in crate::tui) name: String,
    pub(in crate::tui) kind: &'static str,
    pub(in crate::tui) task: String,
    pub(in crate::tui) status: Status,
    pub(in crate::tui) rounds: usize,
    pub(in crate::tui) calls: usize,
    pub(in crate::tui) latest: String,
    /// Stops this subagent only.
    cancel: Arc<AtomicBool>,
    /// The user asked for a cancel that the worker has not confirmed yet.
    pub(in crate::tui) cancelling: bool,
}

#[derive(Default)]
pub(in crate::tui) struct Tracker {
    pub(in crate::tui) entries: Vec<Tracked>,
    pub(in crate::tui) open: bool,
    pub(in crate::tui) selected: usize,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }

    fn color(self) -> Color {
        match self {
            Status::Queued => Color::Gray,
            Status::Running => crate::tui::theme::accent_bright(),
            Status::Done => Color::Rgb(110, 220, 130),
            Status::Failed => Color::Rgb(235, 80, 80),
            Status::Cancelled => Color::Rgb(255, 197, 92),
        }
    }

    fn active(self) -> bool {
        matches!(self, Status::Queued | Status::Running)
    }
}

impl Tracker {
    pub(in crate::tui) fn apply(&mut self, event: SubagentEvent) {
        if let SubagentEvent::Queued {
            id,
            name,
            kind,
            task,
            cancel,
        } = event
        {
            self.entries.push(Tracked {
                id,
                name,
                kind,
                task,
                status: Status::Queued,
                rounds: 0,
                calls: 0,
                latest: String::new(),
                cancel,
                cancelling: false,
            });
            return;
        }
        let id = match &event {
            SubagentEvent::Queued { id, .. }
            | SubagentEvent::Started { id }
            | SubagentEvent::Round { id, .. }
            | SubagentEvent::Action { id, .. }
            | SubagentEvent::Finished { id, .. } => *id,
        };
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        match event {
            SubagentEvent::Queued { .. } => {}
            SubagentEvent::Started { .. } => entry.status = Status::Running,
            SubagentEvent::Round { round, .. } => {
                entry.status = Status::Running;
                entry.rounds = round;
            }
            SubagentEvent::Action { calls, action, .. } => {
                entry.calls = calls;
                entry.latest = action;
            }
            SubagentEvent::Finished { outcome, .. } => {
                entry.status = match outcome {
                    SubagentOutcome::Done => Status::Done,
                    SubagentOutcome::Failed => Status::Failed,
                    SubagentOutcome::Cancelled => Status::Cancelled,
                };
                entry.cancelling = false;
            }
        }
    }

    /// Subagents that are queued or running.
    pub(in crate::tui) fn active(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status.active())
            .count()
    }

    /// Cancels the highlighted subagent if it has not finished; returns its name.
    pub(in crate::tui) fn cancel_selected(&mut self) -> Option<String> {
        let entry = self
            .entries
            .get_mut(self.selected)
            .filter(|entry| entry.status.active())?;
        entry.cancel.store(true, Ordering::SeqCst);
        entry.cancelling = true;
        Some(entry.name.clone())
    }
}

/// The buttons at the bottom of the window. Close is highlighted, so Enter never cancels by
/// accident.
fn buttons() -> Dialog<'static> {
    Dialog {
        id: "tracker",
        title: String::new(),
        tone: Tone::Normal,
        body: Vec::new(),
        buttons: vec![
            Button::new("Cancel subagent", 'x'),
            Button::on_key("Close", KeyCode::Esc),
        ],
        cancel: 1,
        default: 1,
    }
}

impl App {
    pub(super) fn open_tracker(&mut self) {
        self.tracker.open = true;
        self.tracker.selected = self
            .tracker
            .selected
            .min(self.tracker.entries.len().saturating_sub(1));
    }

    pub(super) fn handle_tracker_key(&mut self, key: KeyEvent) {
        match route(&buttons(), &mut self.dialog_focus, key) {
            Routed::Press(KeyCode::Char('x')) => {
                self.notice = match self.tracker.cancel_selected() {
                    Some(name) => format!("Cancelling subagent {name}; the others go on."),
                    None => "That subagent has already finished.".to_owned(),
                };
            }
            Routed::Press(_) => self.tracker.open = false,
            Routed::Moved => {}
            Routed::Other => match key.code {
                KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.tracker.open = false
                }
                KeyCode::Up => self.tracker.selected = self.tracker.selected.saturating_sub(1),
                KeyCode::Down => {
                    self.tracker.selected = (self.tracker.selected + 1)
                        .min(self.tracker.entries.len().saturating_sub(1))
                }
                _ => {}
            },
        }
    }
}

/// The status-line hint while subagents are at work.
pub(in crate::tui) fn status_hint(tracker: &Tracker) -> Option<String> {
    let active = tracker.active();
    (active > 0).then(|| {
        format!(
            "{active} subagent{} running · Shift+↓ to see them",
            if active == 1 { "" } else { "s" }
        )
    })
}

/// Cuts `text` to `width` columns, ending in … when it was longer.
fn fit(text: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthChar as _;
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars().filter(|c| !c.is_control()) {
        let w = character.width().unwrap_or(0);
        if used + w + 1 > width {
            out.push('…');
            return out;
        }
        out.push(character);
        used += w;
    }
    out
}

fn row_line(entry: &Tracked, selected: bool, width: usize) -> Line<'static> {
    let status = if entry.cancelling && entry.status.active() {
        "cancelling"
    } else {
        entry.status.word()
    };
    let progress = match entry.status {
        Status::Queued => "waiting".to_owned(),
        _ => format!(
            "round {} · {} call{}",
            entry.rounds,
            entry.calls,
            if entry.calls == 1 { "" } else { "s" }
        ),
    };
    let label = fit(&format!("{} · {}", entry.kind, entry.name), LABEL_WIDTH - 1);
    let used = 2 + STATUS_WIDTH + LABEL_WIDTH + PROGRESS_WIDTH;
    let accent = crate::tui::theme::accent_bright();
    Line::from(vec![
        Span::styled(
            if selected { "▸ " } else { "  " },
            Style::default().fg(accent),
        ),
        Span::styled(
            format!("{status:<STATUS_WIDTH$}"),
            Style::default().fg(entry.status.color()),
        ),
        Span::styled(
            format!("{label:<LABEL_WIDTH$}"),
            if selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
        Span::styled(
            format!("{progress:<PROGRESS_WIDTH$}"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            fit(&entry.latest, width.saturating_sub(used)),
            Style::default().fg(Color::Rgb(200, 205, 212)),
        ),
    ])
}

pub(in crate::tui) fn draw_tracker(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let tracker = &app.tracker;
    let width = area
        .width
        .saturating_sub(4)
        .clamp(50.min(area.width), 116)
        .min(area.width);
    let text_width = width.saturating_sub(4);
    let selected = tracker.entries.get(tracker.selected);
    let task_lines = selected
        .map(|entry| {
            vec![Line::from(vec![
                Span::styled("Task: ", Style::default().fg(Color::Gray)),
                Span::styled(entry.task.clone(), Style::default().fg(Color::White)),
            ])]
        })
        .unwrap_or_default();
    let task_rows = text_rows(&task_lines, text_width).min(TASK_ROWS);
    let list_rows = tracker.entries.len().max(1) as u16;
    // Borders, the summary and a gap, the list, a gap, the task, a gap and the buttons.
    let wanted = 2 + 2 + list_rows + 1 + task_rows + 1 + 1;
    let height = wanted.min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = window("Subagents · this turn", Tone::Normal)
        .style(Style::default().bg(crate::tui::theme::dialog()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 3 {
        return;
    }
    app.hits.wheel_arrows(popup);
    let count = |status: Status| {
        tracker
            .entries
            .iter()
            .filter(|entry| entry.status == status)
            .count()
    };
    let summary = format!(
        "{} running · {} queued · {} done · {} failed · {} cancelled",
        count(Status::Running),
        count(Status::Queued),
        count(Status::Done),
        count(Status::Failed),
        count(Status::Cancelled)
    );
    frame.render_widget(
        Paragraph::new(Span::styled(summary, Style::default().fg(Color::Gray))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let room = inner.height.saturating_sub(2 + 1 + task_rows + 2).max(1);
    let list = Rect::new(inner.x, inner.y + 2, inner.width, room.min(list_rows));
    if tracker.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No subagents have run in this turn.",
                Style::default().fg(Color::DarkGray),
            )),
            list,
        );
    } else {
        let start = (tracker.selected + 1).saturating_sub(list.height as usize);
        let lines = tracker
            .entries
            .iter()
            .enumerate()
            .skip(start)
            .take(list.height as usize)
            .map(|(index, entry)| {
                app.hits.click(
                    line_rect(list, index - start),
                    Click::Row(MouseRow::new(index, tracker.selected).activate(None)),
                );
                row_line(entry, index == tracker.selected, inner.width as usize)
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(lines), list);
    }
    let task_area = Rect::new(
        inner.x,
        list.bottom() + 1,
        inner.width,
        task_rows.min(inner.bottom().saturating_sub(list.bottom() + 2)),
    );
    frame.render_widget(
        Paragraph::new(task_lines).wrap(Wrap { trim: false }),
        task_area,
    );
    let dialog = buttons();
    draw_buttons(
        frame,
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        &dialog.buttons,
        app.dialog_focus.get(&dialog),
        Tone::Normal,
        "↑/↓ choose · x cancel one · Shift+↑ or Esc close",
        &app.hits,
    );
}

#[cfg(test)]
mod tests {
    use super::{Status, Tracker};
    use crate::Settings;
    use crate::agent::PendingEvent;
    use crate::tui::handle_key;
    use crate::tui::mouse::testing::{click_text, drawn, rows};
    use crate::tui::state::{App, StreamingTurn, TranscriptEntry, TranscriptKind};
    use crate::workflow::{SubagentEvent, SubagentOutcome};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    fn queued(id: usize, name: &str, task: &str) -> (SubagentEvent, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        (
            SubagentEvent::Queued {
                id,
                name: name.to_owned(),
                kind: "explore",
                task: task.to_owned(),
                cancel: cancel.clone(),
            },
            cancel,
        )
    }

    #[test]
    fn events_move_a_subagent_from_queued_to_done() {
        let mut tracker = Tracker::default();
        let (event, _) = queued(0, "auth", "look at auth");
        tracker.apply(event);
        assert_eq!(tracker.entries[0].status, Status::Queued);
        assert_eq!(tracker.active(), 1);
        tracker.apply(SubagentEvent::Started { id: 0 });
        tracker.apply(SubagentEvent::Round { id: 0, round: 2 });
        tracker.apply(SubagentEvent::Action {
            id: 0,
            calls: 3,
            action: "read_file · src/auth.rs".to_owned(),
        });
        let entry = &tracker.entries[0];
        assert_eq!(entry.status, Status::Running);
        assert_eq!((entry.rounds, entry.calls), (2, 3));
        assert_eq!(entry.latest, "read_file · src/auth.rs");
        tracker.apply(SubagentEvent::Finished {
            id: 0,
            outcome: SubagentOutcome::Done,
        });
        assert_eq!(tracker.entries[0].status, Status::Done);
        assert_eq!(tracker.active(), 0);
        tracker.apply(SubagentEvent::Round { id: 7, round: 1 });
        assert_eq!(tracker.entries.len(), 1, "unknown subagents are ignored");
    }

    #[test]
    fn cancel_sets_only_the_selected_subagents_flag() {
        let mut tracker = Tracker::default();
        let (first, first_flag) = queued(0, "one", "a");
        let (second, second_flag) = queued(1, "two", "b");
        tracker.apply(first);
        tracker.apply(second);
        tracker.selected = 1;
        assert_eq!(tracker.cancel_selected().as_deref(), Some("two"));
        assert!(second_flag.load(Ordering::SeqCst));
        assert!(!first_flag.load(Ordering::SeqCst));
        assert!(tracker.entries[1].cancelling);
        tracker.apply(SubagentEvent::Finished {
            id: 1,
            outcome: SubagentOutcome::Cancelled,
        });
        assert_eq!(tracker.entries[1].status, Status::Cancelled);
        assert_eq!(tracker.cancel_selected(), None, "a finished one cannot be");
    }

    fn working_app() -> (App, mpsc::Sender<PendingEvent>, Arc<AtomicBool>) {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: "split it up".to_owned(),
        });
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        let turn_cancel = Arc::new(AtomicBool::new(false));
        app.streaming = Some(StreamingTurn::new(turn_cancel.clone()));
        (app, sender, turn_cancel)
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        handle_key(app, KeyEvent::new(code, modifiers)).expect("key");
    }

    #[test]
    fn the_window_opens_with_shift_down_updates_live_and_cancels_one_subagent() {
        let (mut app, sender, turn_cancel) = working_app();
        let (first, first_flag) = queued(0, "auth module", "look at the auth module");
        let (second, second_flag) = queued(1, "tests", "find the failing tests");
        sender.send(PendingEvent::Subagent(first)).unwrap();
        sender.send(PendingEvent::Subagent(second)).unwrap();
        sender
            .send(PendingEvent::Subagent(SubagentEvent::Started { id: 0 }))
            .unwrap();
        app.poll_response();
        let status = rows(&drawn(&app, 100, 30)).join("\n");
        assert!(status.contains("2 subagents running"), "{status}");
        assert!(status.contains("Shift+↓"), "{status}");

        press(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
        assert!(app.tracker.open);
        let shown = rows(&drawn(&app, 100, 30)).join("\n");
        for expected in [
            "auth module",
            "tests",
            "running",
            "queued",
            "look at the auth module",
        ] {
            assert!(shown.contains(expected), "{expected} missing:\n{shown}");
        }

        sender
            .send(PendingEvent::Subagent(SubagentEvent::Action {
                id: 0,
                calls: 1,
                action: "read_file · src/auth.rs".to_owned(),
            }))
            .unwrap();
        app.poll_response();
        let shown = rows(&drawn(&app, 100, 30)).join("\n");
        assert!(shown.contains("read_file · src/auth.rs"), "live: {shown}");

        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(second_flag.load(Ordering::SeqCst), "the selected one");
        assert!(!first_flag.load(Ordering::SeqCst), "not the other");
        assert!(!turn_cancel.load(Ordering::SeqCst), "not the turn");
        assert!(app.streaming.is_some());

        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        assert!(!app.tracker.open, "Shift+Up closes");
        press(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.tracker.open, "Esc closes");
        assert!(app.streaming.is_some(), "and does not cancel the turn");
        assert!(!turn_cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn the_cancel_button_and_rows_take_clicks() {
        let (mut app, sender, _) = working_app();
        let (first, first_flag) = queued(0, "alpha task", "a");
        let (second, _) = queued(1, "bravo task", "b");
        sender.send(PendingEvent::Subagent(first)).unwrap();
        sender.send(PendingEvent::Subagent(second)).unwrap();
        app.poll_response();
        app.tracker.open = true;
        app.tracker.selected = 1;
        click_text(&mut app, "alpha task");
        assert_eq!(app.tracker.selected, 0);
        click_text(&mut app, "Cancel subagent");
        assert!(first_flag.load(Ordering::SeqCst));
        assert!(app.tracker.open, "the window stays open to watch it stop");
    }

    #[test]
    fn a_new_turn_starts_an_empty_list() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        let (event, _) = queued(0, "old", "x");
        app.tracker.apply(event);
        app.tracker.open = true;
        app.input = "hello".to_owned();
        app.submit().expect("submit");
        assert!(app.tracker.entries.is_empty());
        assert!(!app.tracker.open);
    }
}
