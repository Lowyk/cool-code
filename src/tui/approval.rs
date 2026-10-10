//! The approval card: when the model asks permission for an action, the request is shown as a
//! card just above the prompt, with the reason Auto mode gave (if any), the details of the
//! action, and Allow / Deny buttons.
//!
//! Every answer means what it always did: y, Enter on Allow, or a click on Allow approves; n,
//! Esc, Enter on Deny, or a click on Deny declines. Nothing else answers.

use crate::agent::{AUTO_REASON_PREFIX, ToolApproval};
use crate::tui::dialog::{Button, Dialog, Routed, Tone, draw_buttons, route, text_rows, window};
use crate::tui::mouse::Click;
use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph, Wrap};

/// Rows of details the card shows before it needs scrolling or "show all".
const DETAIL_ROWS: u16 = 10;
/// Rows the card needs besides its text: two borders, a gap and the buttons.
const CHROME_ROWS: u16 = 4;
/// How far PageUp and PageDown move.
const PAGE: u16 = 8;

/// The buttons of the card: Allow is highlighted, so Enter approves as it always has.
fn buttons() -> Dialog<'static> {
    Dialog {
        id: "approval",
        title: String::new(),
        tone: Tone::Warning,
        body: Vec::new(),
        buttons: vec![Button::new("Allow", 'y'), Button::new("Deny", 'n')],
        cancel: 1,
        default: 0,
    }
}

/// Auto mode puts its reason in front of the details; the card shows it on a line of its own.
fn split_reason(details: &str) -> (Option<&str>, &str) {
    match details
        .strip_prefix(AUTO_REASON_PREFIX)
        .and_then(|rest| rest.split_once("\n\n"))
    {
        Some((reason, rest)) => (Some(reason), rest),
        None => (None, details),
    }
}

/// The details as lines, with the lines a change removes in red and the ones it adds in green.
fn detail_lines(details: &str) -> Vec<Line<'static>> {
    details
        .lines()
        .map(|line| {
            let color = if line.starts_with("+ ") || line == "+" {
                Color::Rgb(110, 220, 130)
            } else if line.starts_with("- ") || line == "-" {
                Color::Rgb(255, 120, 120)
            } else {
                Color::Rgb(200, 205, 212)
            };
            Line::from(Span::styled(line.to_owned(), Style::default().fg(color)))
        })
        .collect()
}

impl App {
    pub(super) fn handle_approval_key(&mut self, key: KeyEvent) {
        if self.tool_approval.is_none() {
            return;
        }
        match route(&buttons(), &mut self.dialog_focus, key) {
            Routed::Press(KeyCode::Char('y')) => self.answer_approval(true),
            Routed::Press(_) => self.answer_approval(false),
            Routed::Moved => {}
            Routed::Other => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.approval_scroll = self.approval_scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => self.scroll_approval(1),
                KeyCode::PageUp => self.approval_scroll = self.approval_scroll.saturating_sub(PAGE),
                KeyCode::PageDown => self.scroll_approval(PAGE),
                KeyCode::Home => self.approval_scroll = 0,
                KeyCode::Char('v' | 'V') => self.approval_expanded = !self.approval_expanded,
                _ => {}
            },
        }
    }

    /// Scrolls down, never past the last line of the details (the drawing keeps the last
    /// screenful in view).
    fn scroll_approval(&mut self, by: u16) {
        let lines = self
            .tool_approval
            .as_ref()
            .map_or(0, |approval| approval.details.lines().count());
        self.approval_scroll = self
            .approval_scroll
            .saturating_add(by)
            .min(lines.min(u16::MAX as usize) as u16);
    }

    fn answer_approval(&mut self, approved: bool) {
        let Some(approval) = self.tool_approval.take() else {
            return;
        };
        let _ = approval.response.send(approved);
        self.transcript.push(TranscriptEntry {
            kind: TranscriptKind::CommandOutput,
            text: format!(
                "{}: {}",
                if approved { "Approved" } else { "Declined" },
                approval.title
            ),
        });
        self.approval_scroll = 0;
        self.approval_expanded = false;
    }
}

/// Draws the card over the bottom of the conversation, just above `prompt`, as wide as it.
pub(super) fn draw_approval_card(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    prompt: Rect,
    app: &App,
    approval: &ToolApproval,
) {
    let room = prompt.y.saturating_sub(area.y);
    if room < CHROME_ROWS + 1 || prompt.width < 20 {
        return;
    }
    let (reason, details) = split_reason(&approval.details);
    let text_width = prompt.width.saturating_sub(4);
    let reason_lines = reason
        .map(|reason| {
            vec![Line::from(vec![
                Span::styled(
                    "Auto mode asks because: ",
                    Style::default().fg(Color::Rgb(255, 197, 92)),
                ),
                Span::styled(
                    reason.to_owned(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ])]
        })
        .unwrap_or_default();
    let reason_rows = text_rows(&reason_lines, text_width);
    let lines = detail_lines(details);
    let detail_rows = text_rows(&lines, text_width);
    let wanted_details = if app.approval_expanded {
        detail_rows
    } else {
        detail_rows.min(DETAIL_ROWS)
    };
    let height = (reason_rows + wanted_details + CHROME_ROWS).min(room);
    let card = Rect::new(prompt.x, prompt.y - height, prompt.width, height);
    frame.render_widget(Clear, card);
    let block = window(format!("Approve · {}", approval.title), Tone::Warning)
        .style(Style::default().bg(crate::tui::theme::dialog()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    app.hits.wheel_arrows(card);
    let reason_area = Rect {
        height: reason_rows.min(inner.height),
        ..inner
    };
    frame.render_widget(
        Paragraph::new(reason_lines).wrap(Wrap { trim: false }),
        reason_area,
    );
    let details_area = Rect::new(
        inner.x,
        inner.y + reason_area.height,
        inner.width,
        inner
            .height
            .saturating_sub(reason_area.height)
            .saturating_sub(2),
    );
    let last_scroll = detail_rows.saturating_sub(details_area.height);
    let scroll = app.approval_scroll.min(last_scroll);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        details_area,
    );
    let hidden = detail_rows > details_area.height;
    let hint = if hidden {
        format!(
            "↑/↓ scroll · v {} · rows {}–{} of {detail_rows}",
            if app.approval_expanded {
                "show less"
            } else {
                "show all"
            },
            scroll + 1,
            (scroll + details_area.height).min(detail_rows)
        )
    } else if app.approval_expanded {
        "v show less".to_owned()
    } else {
        String::new()
    };
    let row = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
    if !hint.is_empty() {
        // Recorded before the buttons, which lie on top of it if the row is too narrow.
        let width = unicode_width::UnicodeWidthStr::width(hint.as_str()) as u16;
        app.hits.click(
            Rect::new(row.x, row.y, width.min(row.width), 1),
            Click::Key(KeyCode::Char('v')),
        );
    }
    let dialog = buttons();
    draw_buttons(
        frame,
        row,
        &dialog.buttons,
        app.dialog_focus.get(&dialog),
        Tone::Warning,
        &hint,
        &app.hits,
    );
}

#[cfg(test)]
mod tests {
    use crate::Settings;
    use crate::agent::ToolApproval;
    use crate::tui::handle_key;
    use crate::tui::mouse::testing::{click, find, rows, wheel};
    use crate::tui::render::draw;
    use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::mpsc;

    fn app_asking(title: &str, details: &str) -> (App, mpsc::Receiver<bool>) {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: "please run the tests".to_owned(),
        });
        let (response, decision) = mpsc::sync_channel(1);
        app.tool_approval = Some(ToolApproval {
            title: title.to_owned(),
            details: details.to_owned(),
            response,
        });
        (app, decision)
    }

    fn screen(app: &App) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        terminal
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    }

    const COMMAND: &str = "Permission mode: Manual\nWorking directory: /tmp/x\nShell: POSIX sh -lc\n\nExact command to execute:\ncargo test";

    #[test]
    fn the_card_sits_just_above_the_prompt_input() {
        let (app, _decision) = app_asking("Run shell command", COMMAND);
        let terminal = screen(&app);
        let (_, buttons) = find(&terminal, "[ Allow (y) ]");
        let (_, deny) = find(&terminal, "[ Deny (n) ]");
        assert_eq!(buttons, deny);
        let (_, prompt) = find(&terminal, "Describe what you want");
        // The card's bottom border, then the prompt's top padding, then its first line.
        assert_eq!(prompt, buttons + 3, "{}", rows(&terminal).join("\n"));
        let (_, title) = find(&terminal, "Run shell command");
        // Six rows of details and the card's chrome, ending right above the prompt.
        assert_eq!(title, buttons - 8, "not centered over the conversation");
        assert!(rows(&terminal).join("\n").contains("cargo test"));
    }

    #[test]
    fn auto_modes_reason_gets_its_own_line() {
        let details = format!("Auto mode is asking because: it deletes files\n\n{COMMAND}");
        let (app, _decision) = app_asking("Run shell command", &details);
        let terminal = screen(&app);
        let shown = rows(&terminal).join("\n");
        assert!(shown.contains("it deletes files"), "{shown}");
        assert!(!shown.contains("Auto mode is asking because"), "{shown}");
        let (_, reason) = find(&terminal, "it deletes files");
        let (_, mode) = find(&terminal, "Permission mode: Manual");
        assert!(reason < mode, "the reason comes first");
    }

    #[test]
    fn yes_enter_and_a_click_on_allow_approve() {
        for how in ["y", "Y", "Enter", "click"] {
            let (mut app, decision) = app_asking("Run shell command", COMMAND);
            match how {
                "y" => press(&mut app, KeyCode::Char('y')),
                "Y" => press(&mut app, KeyCode::Char('Y')),
                "Enter" => press(&mut app, KeyCode::Enter),
                _ => {
                    let terminal = screen(&app);
                    let (x, y) = find(&terminal, "Allow");
                    crate::tui::mouse::handle_mouse(&mut app, click(x, y)).expect("click");
                }
            }
            assert_eq!(decision.try_recv(), Ok(true), "{how}");
            assert!(app.tool_approval.is_none(), "{how}");
            assert_eq!(
                app.transcript.last().map(|entry| entry.text.as_str()),
                Some("Approved: Run shell command"),
                "{how}"
            );
        }
    }

    #[test]
    fn no_esc_deny_and_a_click_on_deny_decline() {
        for how in ["n", "N", "Esc", "Tab Enter", "click"] {
            let (mut app, decision) = app_asking("Edit src/main.rs", COMMAND);
            match how {
                "n" => press(&mut app, KeyCode::Char('n')),
                "N" => press(&mut app, KeyCode::Char('N')),
                "Esc" => press(&mut app, KeyCode::Esc),
                "Tab Enter" => {
                    press(&mut app, KeyCode::Tab);
                    assert!(app.tool_approval.is_some(), "Tab only moves");
                    press(&mut app, KeyCode::Enter);
                }
                _ => {
                    let terminal = screen(&app);
                    let (x, y) = find(&terminal, "Deny");
                    crate::tui::mouse::handle_mouse(&mut app, click(x, y)).expect("click");
                }
            }
            assert_eq!(decision.try_recv(), Ok(false), "{how}");
            assert_eq!(
                app.transcript.last().map(|entry| entry.text.as_str()),
                Some("Declined: Edit src/main.rs"),
                "{how}"
            );
        }
    }

    #[test]
    fn other_keys_never_answer() {
        let (mut app, decision) = app_asking("Run shell command", COMMAND);
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('v'),
            KeyCode::Char('x'),
            KeyCode::PageDown,
        ] {
            press(&mut app, code);
        }
        assert!(decision.try_recv().is_err(), "still waiting");
        assert!(app.tool_approval.is_some());
    }

    #[test]
    fn long_details_are_cut_with_a_way_to_scroll_and_see_everything() {
        let details = (1..=60)
            .map(|line| format!("line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (mut app, _decision) = app_asking("Approve execution plan", &details);
        let shown = rows(&screen(&app)).join("\n");
        assert!(shown.contains("line 01"), "{shown}");
        assert!(!shown.contains("line 60"), "cut: {shown}");
        assert!(shown.contains("show all"), "{shown}");
        for _ in 0..80 {
            press(&mut app, KeyCode::Down);
        }
        let shown = rows(&screen(&app)).join("\n");
        assert!(shown.contains("line 60"), "scrolled to the end: {shown}");
        for _ in 0..80 {
            press(&mut app, KeyCode::Up);
        }
        let short = rows(&screen(&app))
            .iter()
            .filter(|row| row.contains("line "))
            .count();
        press(&mut app, KeyCode::Char('v'));
        let tall = rows(&screen(&app))
            .iter()
            .filter(|row| row.contains("line "))
            .count();
        assert!(tall > short, "v shows more: {short} -> {tall}");
        // The wheel over the card scrolls it.
        let terminal = screen(&app);
        let (x, y) = find(&terminal, "line 01");
        crate::tui::mouse::handle_mouse(&mut app, wheel(x, y, false)).expect("wheel");
        assert!(!rows(&screen(&app)).join("\n").contains("line 01"));
    }
}
