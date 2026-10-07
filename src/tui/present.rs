//! Putting a finished frame on the terminal without flicker or a wandering cursor.
//!
//! `Terminal::draw` writes every changed cell while the real cursor is still visible, so the
//! cursor visibly jumps from cell to cell, and it re-places the cursor on every frame, which
//! restarts its blink. The presenter instead
//!
//! * wraps each frame in a synchronized update, so terminals that support it show the frame
//!   all at once (others ignore the request),
//! * hides the cursor only while cells are being written, and
//! * leaves the cursor alone when neither the screen nor the cursor changed.

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::queue;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use std::io::{self, Write};

pub(super) struct Presenter {
    /// The last frame that reached the screen.
    last: Option<Buffer>,
    cursor_visible: bool,
    /// Where the real cursor is, when known.
    cursor_at: Option<Position>,
}

impl Presenter {
    pub(super) fn new() -> Presenter {
        Presenter {
            last: None,
            // A terminal starts with a visible cursor at an unknown place.
            cursor_visible: true,
            cursor_at: None,
        }
    }

    /// Renders one frame. `wanted` is asked for the cursor position after rendering, because
    /// the render itself decides where the cursor belongs.
    pub(super) fn present<W: Write>(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<W>>,
        render: impl FnOnce(&mut ratatui::Frame<'_>),
        wanted: impl FnOnce() -> Option<Position>,
    ) -> io::Result<()> {
        terminal.autoresize()?;
        {
            let mut frame = terminal.get_frame();
            render(&mut frame);
        }
        let wanted = wanted();
        let current = terminal.current_buffer_mut().clone();
        let changed = self.last.as_ref() != Some(&current);
        let cursor_moves = match wanted {
            Some(position) => !self.cursor_visible || self.cursor_at != Some(position),
            None => self.cursor_visible,
        };
        let busy = changed || cursor_moves;
        if busy {
            queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
        }
        if changed {
            if self.cursor_visible {
                queue!(terminal.backend_mut(), Hide)?;
                self.cursor_visible = false;
            }
            terminal.flush()?;
            // Writing cells moved the cursor.
            self.cursor_at = None;
            self.last = Some(current);
        }
        terminal.swap_buffers();
        match wanted {
            Some(position) => {
                if !self.cursor_visible {
                    queue!(terminal.backend_mut(), Show)?;
                    self.cursor_visible = true;
                }
                if self.cursor_at != Some(position) {
                    queue!(terminal.backend_mut(), MoveTo(position.x, position.y))?;
                    self.cursor_at = Some(position);
                }
            }
            None => {
                if self.cursor_visible {
                    queue!(terminal.backend_mut(), Hide)?;
                    self.cursor_visible = false;
                }
            }
        }
        if busy {
            queue!(terminal.backend_mut(), EndSynchronizedUpdate)?;
        }
        terminal.backend_mut().flush()
    }
}

#[cfg(test)]
mod tests {
    use super::Presenter;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::{Position, Rect};
    use ratatui::widgets::Paragraph;
    use ratatui::{Terminal, TerminalOptions, Viewport};

    const BEGIN: &str = "\x1b[?2026h";
    const END: &str = "\x1b[?2026l";
    const HIDE: &str = "\x1b[?25l";
    const SHOW: &str = "\x1b[?25h";

    /// A terminal writer whose bytes the test can read back.
    #[derive(Clone, Default)]
    struct Wire(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

    impl std::io::Write for Wire {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    type TestTerminal = (Terminal<CrosstermBackend<Wire>>, Wire);

    fn new_terminal() -> TestTerminal {
        let wire = Wire::default();
        let terminal = Terminal::with_options(
            CrosstermBackend::new(wire.clone()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 20, 3)),
            },
        )
        .expect("terminal");
        (terminal, wire)
    }

    /// Everything written since the last call.
    fn sent(wire: &Wire) -> String {
        let bytes = std::mem::take(&mut *wire.0.borrow_mut());
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn show(
        presenter: &mut Presenter,
        (terminal, wire): &mut TestTerminal,
        text: &'static str,
        cursor: Option<Position>,
    ) -> String {
        presenter
            .present(
                terminal,
                |frame| {
                    let area = frame.area();
                    frame.render_widget(Paragraph::new(text), area);
                },
                || cursor,
            )
            .expect("present");
        sent(wire)
    }

    #[test]
    fn a_changed_frame_is_one_synchronized_update_with_the_cursor_hidden_while_drawing() {
        let (mut presenter, mut terminal) = (Presenter::new(), new_terminal());
        let out = show(&mut presenter, &mut terminal, "HELLO", None);
        assert!(out.starts_with(BEGIN), "{out:?}");
        assert!(out.ends_with(END), "{out:?}");
        let hide = out.find(HIDE).expect("hidden");
        let content = out.find("HELLO").expect("drawn");
        assert!(
            hide < content,
            "the cursor must be hidden before cells are written"
        );
        assert!(!out.contains(SHOW), "no cursor wanted: {out:?}");
    }

    #[test]
    fn the_cursor_comes_back_where_it_belongs_after_the_cells_are_written() {
        let (mut presenter, mut terminal) = (Presenter::new(), new_terminal());
        let out = show(
            &mut presenter,
            &mut terminal,
            "HELLO",
            Some(Position::new(3, 1)),
        );
        let content = out.find("HELLO").expect("drawn");
        let shown = out.rfind(SHOW).expect("cursor shown");
        let moved = out.rfind("\x1b[2;4H").expect("moved to row 2, column 4");
        assert!(content < shown && shown < moved, "{out:?}");
    }

    #[test]
    fn an_unchanged_frame_writes_nothing_at_all() {
        let (mut presenter, mut terminal) = (Presenter::new(), new_terminal());
        show(
            &mut presenter,
            &mut terminal,
            "HELLO",
            Some(Position::new(3, 1)),
        );
        for _ in 0..3 {
            let out = show(
                &mut presenter,
                &mut terminal,
                "HELLO",
                Some(Position::new(3, 1)),
            );
            assert_eq!(out, "", "an idle frame must not touch the terminal");
        }
        let (mut presenter, mut terminal) = (Presenter::new(), new_terminal());
        show(&mut presenter, &mut terminal, "HELLO", None);
        assert_eq!(show(&mut presenter, &mut terminal, "HELLO", None), "");
    }

    #[test]
    fn moving_only_the_cursor_moves_only_the_cursor() {
        let (mut presenter, mut terminal) = (Presenter::new(), new_terminal());
        show(
            &mut presenter,
            &mut terminal,
            "HELLO",
            Some(Position::new(3, 1)),
        );
        let out = show(
            &mut presenter,
            &mut terminal,
            "HELLO",
            Some(Position::new(5, 2)),
        );
        assert!(out.contains("\x1b[3;6H"), "{out:?}");
        assert!(!out.contains(HIDE) && !out.contains(SHOW), "{out:?}");
        assert!(!out.contains("HELLO"), "nothing was redrawn: {out:?}");
    }

    #[test]
    fn a_visible_cursor_is_hidden_for_the_redraw_and_restored_after() {
        let (mut presenter, mut terminal) = (Presenter::new(), new_terminal());
        show(
            &mut presenter,
            &mut terminal,
            "HELLO",
            Some(Position::new(3, 1)),
        );
        let out = show(
            &mut presenter,
            &mut terminal,
            "WXYZQ",
            Some(Position::new(3, 1)),
        );
        let hide = out.find(HIDE).expect("hidden for the redraw");
        let content = out.find("WXYZQ").expect("redrawn");
        let shown = out.rfind(SHOW).expect("restored");
        assert!(hide < content && content < shown, "{out:?}");
        assert!(
            out.rfind("\x1b[2;4H").is_some_and(|at| at > shown),
            "{out:?}"
        );
    }

    #[test]
    fn the_cursor_is_hidden_once_and_not_re_sent_every_frame() {
        let (mut presenter, mut terminal) = (Presenter::new(), new_terminal());
        show(
            &mut presenter,
            &mut terminal,
            "ONE",
            Some(Position::new(1, 0)),
        );
        let hidden = show(&mut presenter, &mut terminal, "TWO", None);
        assert_eq!(hidden.matches(HIDE).count(), 1, "{hidden:?}");
        let again = show(&mut presenter, &mut terminal, "THREE", None);
        assert!(!again.contains(SHOW), "{again:?}");
        assert_eq!(again.matches(HIDE).count(), 0, "already hidden: {again:?}");
    }
}
