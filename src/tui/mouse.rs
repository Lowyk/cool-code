//! Mouse support. While a frame is drawn, every clickable or scrollable part of the screen records
//! where it is and what a click or the wheel there does, so a click is handled against exactly
//! what the user saw instead of a second copy of the layout.
//!
//! A click on a list row moves the highlight there; a click on the row that is already
//! highlighted (so also the second click of a double-click) activates it, as Enter would.

use crate::tui::state::App;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::cell::RefCell;

/// What a click on a region does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Click {
    /// Presses a key, as a button's letter does.
    Key(KeyCode),
    /// One row of a list that is moved through with the arrow keys.
    Row(Row),
}

/// A clickable list row, described by the keys that reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Row {
    /// The row's position among the rows the arrow keys step through.
    pub(in crate::tui) index: usize,
    /// The highlighted row's position when the frame was drawn.
    pub(in crate::tui) current: usize,
    /// The list is moved through with ←/→ instead of ↑/↓.
    pub(in crate::tui) horizontal: bool,
    /// Pressed when the highlighted row is clicked; `None` when rows only select.
    pub(in crate::tui) activate: Option<KeyCode>,
    /// Pressed first when the list does not have the keyboard focus yet.
    pub(in crate::tui) focus: Option<KeyCode>,
}

impl Row {
    pub(in crate::tui) fn new(index: usize, current: usize) -> Row {
        Row {
            index,
            current,
            horizontal: false,
            activate: Some(KeyCode::Enter),
            focus: None,
        }
    }

    pub(in crate::tui) fn horizontal(self) -> Row {
        Row {
            horizontal: true,
            ..self
        }
    }

    pub(in crate::tui) fn activate(self, key: Option<KeyCode>) -> Row {
        Row {
            activate: key,
            ..self
        }
    }

    pub(in crate::tui) fn focus(self, key: Option<KeyCode>) -> Row {
        Row { focus: key, ..self }
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Click(Click),
    Wheel {
        up: KeyEvent,
        down: KeyEvent,
    },
    /// A window that takes every key: what lies under it cannot be clicked or scrolled.
    Modal,
}

/// The regions of the last frame, in drawing order (later ones lie on top).
#[derive(Default)]
pub(in crate::tui) struct Hits(RefCell<Vec<(Rect, Action)>>);

impl Hits {
    pub(in crate::tui) fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    pub(in crate::tui) fn click(&self, area: Rect, click: Click) {
        if area.width > 0 && area.height > 0 {
            self.0.borrow_mut().push((area, Action::Click(click)));
        }
    }

    /// Wheel turns over `area` press `up` or `down`.
    pub(in crate::tui) fn wheel(&self, area: Rect, up: KeyEvent, down: KeyEvent) {
        self.0.borrow_mut().push((area, Action::Wheel { up, down }));
    }

    /// Wheel turns over `area` press ↑ or ↓.
    pub(in crate::tui) fn wheel_arrows(&self, area: Rect) {
        self.wheel(
            area,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
    }

    /// Covers `area`: nothing drawn earlier under it reacts to the mouse.
    pub(in crate::tui) fn modal(&self, area: Rect) {
        self.0.borrow_mut().push((area, Action::Modal));
    }

    #[cfg(test)]
    pub(in crate::tui) fn len(&self) -> usize {
        self.0.borrow().len()
    }

    /// The click handled at a point, from the topmost region that reacts to clicks.
    pub(in crate::tui) fn click_at(&self, x: u16, y: u16) -> Option<Click> {
        let point = Position::new(x, y);
        for (area, action) in self.0.borrow().iter().rev() {
            if !area.contains(point) {
                continue;
            }
            match action {
                Action::Click(click) => return Some(*click),
                Action::Modal => return None,
                Action::Wheel { .. } => {}
            }
        }
        None
    }

    /// The key a wheel turn at a point presses, from the topmost region that scrolls.
    pub(in crate::tui) fn wheel_at(&self, x: u16, y: u16, up: bool) -> Option<KeyEvent> {
        let point = Position::new(x, y);
        for (area, action) in self.0.borrow().iter().rev() {
            if !area.contains(point) {
                continue;
            }
            match action {
                Action::Wheel { up: key_up, down } => {
                    return Some(if up { *key_up } else { *down });
                }
                Action::Modal => return None,
                Action::Click(_) => {}
            }
        }
        None
    }

    /// After a click moved a list's highlight, the other rows of that list measure from the new
    /// position, so a second click before the next frame still lands where it was aimed.
    fn moved(&self, row: Row) {
        for (_, action) in self.0.borrow_mut().iter_mut() {
            if let Action::Click(Click::Row(other)) = action
                && other.current == row.current
                && other.horizontal == row.horizontal
            {
                other.current = row.index;
                other.focus = None;
            }
        }
    }
}

/// The one-row area of line `index` of text drawn from the top of `area`; empty when that line
/// falls below it.
pub(in crate::tui) fn line_rect(area: Rect, index: usize) -> Rect {
    let y = area.y.saturating_add(index.min(u16::MAX as usize) as u16);
    if y >= area.bottom() {
        return Rect::default();
    }
    Rect::new(area.x, y, area.width, 1)
}

/// Records `clicks` (a line index of `lines` and what clicking it does) for text drawn wrapped
/// into `area` from its top, accounting for lines that wrap onto several rows.
pub(in crate::tui) fn record_wrapped(
    hits: &Hits,
    area: Rect,
    lines: &[ratatui::text::Line<'_>],
    clicks: &[(usize, Click)],
) {
    let mut starts = Vec::with_capacity(lines.len());
    let mut row = 0usize;
    for line in lines {
        starts.push(row);
        row += crate::tui::dialog::text_rows(std::slice::from_ref(line), area.width) as usize;
    }
    for (index, click) in clicks {
        if let Some(start) = starts.get(*index) {
            hits.click(line_rect(area, *start), *click);
        }
    }
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Handles one mouse event against the regions of the last frame.
pub(in crate::tui) fn handle_mouse(app: &mut App, event: MouseEvent) -> Result<()> {
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let Some(click) = app.hits.click_at(event.column, event.row) else {
                return Ok(());
            };
            match click {
                Click::Key(code) => {
                    // What was clicked may be gone now; wait for the next frame.
                    app.hits.clear();
                    super::handle_key(app, press(code))?;
                }
                Click::Row(row) => {
                    if let Some(focus) = row.focus {
                        super::handle_key(app, press(focus))?;
                    }
                    if row.index == row.current {
                        if row.focus.is_none()
                            && let Some(activate) = row.activate
                        {
                            app.hits.clear();
                            super::handle_key(app, press(activate))?;
                        }
                        return Ok(());
                    }
                    let step = match (row.horizontal, row.index > row.current) {
                        (false, true) => KeyCode::Down,
                        (false, false) => KeyCode::Up,
                        (true, true) => KeyCode::Right,
                        (true, false) => KeyCode::Left,
                    };
                    for _ in 0..row.index.abs_diff(row.current) {
                        super::handle_key(app, press(step))?;
                    }
                    app.hits.moved(row);
                }
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = event.kind == MouseEventKind::ScrollUp;
            if let Some(key) = app.hits.wheel_at(event.column, event.row, up) {
                super::handle_key(app, key)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
pub(in crate::tui) mod testing {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    pub(in crate::tui) fn click(x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    pub(in crate::tui) fn wheel(x: u16, y: u16, up: bool) -> MouseEvent {
        MouseEvent {
            kind: if up {
                MouseEventKind::ScrollUp
            } else {
                MouseEventKind::ScrollDown
            },
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// The screen as rows of text.
    pub(in crate::tui) fn rows(
        terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
    ) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    /// The position of the first character of `needle` on screen.
    pub(in crate::tui) fn find(
        terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
        needle: &str,
    ) -> (u16, u16) {
        let rows = rows(terminal);
        for (y, row) in rows.iter().enumerate() {
            if let Some(byte) = row.find(needle) {
                let x = row[..byte].chars().count();
                return (x as u16, y as u16);
            }
        }
        panic!("{needle:?} not on screen:\n{}", rows.join("\n"));
    }

    /// Draws the whole app on a test terminal.
    pub(in crate::tui) fn drawn(
        app: &crate::tui::state::App,
        width: u16,
        height: u16,
    ) -> ratatui::Terminal<ratatui::backend::TestBackend> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                .expect("terminal");
        terminal
            .draw(|frame| crate::tui::render::draw(frame, app, 0))
            .expect("draw");
        terminal
    }

    /// Draws the app on a 100 by 30 screen and clicks the first character of `needle`.
    pub(in crate::tui) fn click_text(app: &mut crate::tui::state::App, needle: &str) {
        let terminal = drawn(app, 100, 30);
        let (x, y) = find(&terminal, needle);
        super::handle_mouse(app, click(x, y)).expect("click");
    }

    /// Whether `needle` is drawn as one of the shared dialog's buttons.
    pub(in crate::tui) fn has_button(app: &crate::tui::state::App, label: &str) -> bool {
        rows(&drawn(app, 100, 30))
            .iter()
            .any(|row| row.contains(&format!("[ {label} (")))
    }
}

#[cfg(test)]
mod tests {
    use super::{Click, Hits, Row};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    #[test]
    fn the_topmost_region_takes_the_click_and_a_modal_window_hides_what_is_under_it() {
        let hits = Hits::default();
        hits.click(Rect::new(0, 0, 10, 10), Click::Key(KeyCode::Char('a')));
        hits.click(Rect::new(2, 2, 3, 1), Click::Key(KeyCode::Char('b')));
        assert_eq!(hits.click_at(3, 2), Some(Click::Key(KeyCode::Char('b'))));
        assert_eq!(hits.click_at(8, 8), Some(Click::Key(KeyCode::Char('a'))));
        assert_eq!(hits.click_at(20, 20), None);
        hits.modal(Rect::new(0, 0, 10, 5));
        assert_eq!(hits.click_at(3, 2), None, "covered by the modal window");
        assert_eq!(hits.click_at(8, 8), Some(Click::Key(KeyCode::Char('a'))));
    }

    #[test]
    fn the_wheel_finds_the_topmost_scrollable_region_and_skips_buttons() {
        let hits = Hits::default();
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL);
        hits.wheel(Rect::new(0, 0, 10, 10), up, down);
        hits.click(Rect::new(0, 0, 10, 10), Click::Row(Row::new(1, 0)));
        assert_eq!(hits.wheel_at(1, 1, true), Some(up));
        assert_eq!(hits.wheel_at(1, 1, false), Some(down));
        hits.modal(Rect::new(0, 0, 10, 10));
        assert_eq!(hits.wheel_at(1, 1, true), None);
    }

    #[test]
    fn a_moved_highlight_updates_the_rest_of_the_list() {
        let hits = Hits::default();
        hits.click(Rect::new(0, 0, 5, 1), Click::Row(Row::new(0, 0)));
        hits.click(Rect::new(0, 1, 5, 1), Click::Row(Row::new(1, 0)));
        hits.click(Rect::new(0, 2, 5, 1), Click::Row(Row::new(2, 0)));
        hits.moved(Row::new(2, 0));
        assert_eq!(hits.click_at(0, 1), Some(Click::Row(Row::new(1, 2))));
        assert_eq!(hits.len(), 3);
    }
}
