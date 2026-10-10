//! The one look every confirmation and window shares: a titled box with rounded corners, its
//! text, and at the bottom right a row of buttons. ←/→ or Tab move between the buttons, Enter
//! presses the highlighted one, each button's letter presses it directly, and Esc presses the
//! safe one.

use crate::tui::mouse::{Click, Hits};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

/// The widest a dialog gets; wider lines are harder to read.
const MAX_WIDTH: u16 = 88;
/// The narrowest a dialog gets, screen permitting.
const MIN_WIDTH: u16 = 44;
/// Rows a dialog needs besides its text: two borders, the top padding, a gap and the buttons.
const CHROME_ROWS: u16 = 5;
/// Columns besides the text: two borders and two columns of padding on each side.
const CHROME_COLUMNS: u16 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Tone {
    /// An ordinary window, in the theme's accent.
    Normal,
    /// Something to read before going on, in amber.
    Warning,
    /// Something costly or that cannot be undone, in red.
    Danger,
}

impl Tone {
    pub(in crate::tui) fn color(self) -> Color {
        match self {
            Tone::Normal => crate::tui::theme::accent(),
            Tone::Warning => Color::Rgb(255, 197, 92),
            Tone::Danger => Color::Rgb(235, 80, 80),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Button {
    pub(in crate::tui) label: String,
    /// The key that presses it, and what the dialog hands back when it is pressed.
    pub(in crate::tui) key: KeyCode,
}

impl Button {
    pub(in crate::tui) fn new(label: impl Into<String>, letter: char) -> Button {
        Button {
            label: label.into(),
            key: KeyCode::Char(letter),
        }
    }

    pub(in crate::tui) fn on_key(label: impl Into<String>, key: KeyCode) -> Button {
        Button {
            label: label.into(),
            key,
        }
    }

    /// How the button is drawn: its label and the key that presses it.
    fn text(&self) -> String {
        let key = match self.key {
            KeyCode::Char(letter) => letter.to_string(),
            KeyCode::Esc => "Esc".to_owned(),
            KeyCode::Enter => "Enter".to_owned(),
            other => format!("{other:?}"),
        };
        format!("[ {} ({key}) ]", self.label)
    }
}

pub(in crate::tui) struct Dialog<'a> {
    /// Tells dialogs apart, so a highlighted button is not carried from one to the next.
    pub(in crate::tui) id: &'static str,
    pub(in crate::tui) title: String,
    pub(in crate::tui) tone: Tone,
    pub(in crate::tui) body: Vec<Line<'a>>,
    pub(in crate::tui) buttons: Vec<Button>,
    /// The button Esc presses: the safe choice.
    pub(in crate::tui) cancel: usize,
    /// The button highlighted when the dialog opens.
    pub(in crate::tui) default: usize,
}

impl<'a> Dialog<'a> {
    /// A question with a yes button (`y`) and a no button (`n`); the no button is the safe one
    /// and starts highlighted.
    pub(in crate::tui) fn confirm(
        id: &'static str,
        title: impl Into<String>,
        tone: Tone,
        body: Vec<Line<'a>>,
        yes: &str,
        no: &str,
    ) -> Dialog<'a> {
        Dialog {
            id,
            title: title.into(),
            tone,
            body,
            buttons: vec![Button::new(yes, 'y'), Button::new(no, 'n')],
            cancel: 1,
            default: 1,
        }
    }

    pub(in crate::tui) fn default_button(self, index: usize) -> Dialog<'a> {
        Dialog {
            default: index,
            ..self
        }
    }
}

/// Which button is highlighted in the open dialog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct DialogFocus {
    id: &'static str,
    button: usize,
}

impl DialogFocus {
    pub(in crate::tui) fn get(&self, dialog: &Dialog<'_>) -> usize {
        let last = dialog.buttons.len().saturating_sub(1);
        if !self.id.is_empty() && self.id == dialog.id {
            self.button.min(last)
        } else {
            dialog.default.min(last)
        }
    }

    pub(in crate::tui) fn clear(&mut self) {
        *self = DialogFocus::default();
    }
}

/// What a key did to a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Routed {
    /// A button was pressed; this is its key.
    Press(KeyCode),
    /// The highlight moved.
    Moved,
    /// The key means nothing to the buttons.
    Other,
}

/// Applies a key to the buttons of `dialog`.
pub(in crate::tui) fn route(dialog: &Dialog<'_>, focus: &mut DialogFocus, key: KeyEvent) -> Routed {
    if dialog.buttons.is_empty() {
        return Routed::Other;
    }
    let current = focus.get(dialog);
    let last = dialog.buttons.len() - 1;
    let pressed = |focus: &mut DialogFocus, index: usize| {
        focus.clear();
        Routed::Press(dialog.buttons[index.min(last)].key)
    };
    match key.code {
        KeyCode::Left | KeyCode::BackTab | KeyCode::Right | KeyCode::Tab => {
            let next = if matches!(key.code, KeyCode::Left | KeyCode::BackTab) {
                current.saturating_sub(1)
            } else {
                (current + 1).min(last)
            };
            *focus = DialogFocus {
                id: dialog.id,
                button: next,
            };
            Routed::Moved
        }
        KeyCode::Enter => pressed(focus, current),
        KeyCode::Esc => pressed(focus, dialog.cancel),
        KeyCode::Char(typed) => match dialog.buttons.iter().position(|button| {
            matches!(button.key, KeyCode::Char(letter) if letter.eq_ignore_ascii_case(&typed))
        }) {
            Some(index) => pressed(focus, index),
            None => Routed::Other,
        },
        _ => Routed::Other,
    }
}

/// How many rows `body` takes when wrapped to `width` columns.
pub(in crate::tui) fn text_rows(body: &[Line<'_>], width: u16) -> u16 {
    let rows = Paragraph::new(body.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.max(1));
    rows.min(u16::MAX as usize) as u16
}

/// Where the dialog sits on a screen of `area`: wide, centered, and as tall as its text.
pub(in crate::tui) fn dialog_area(area: Rect, dialog: &Dialog<'_>) -> Rect {
    let width = area
        .width
        .saturating_sub(8)
        .clamp(MIN_WIDTH.min(area.width), MAX_WIDTH)
        .min(area.width);
    let rows = text_rows(&dialog.body, width.saturating_sub(CHROME_COLUMNS));
    let height = rows.saturating_add(CHROME_ROWS).min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// Draws a dialog in the middle of `area` and records its buttons for the mouse. Everything
/// under it stops reacting to the mouse while it is open.
pub(in crate::tui) fn draw_dialog(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    dialog: &Dialog<'_>,
    focus: &DialogFocus,
    hits: &Hits,
) -> Rect {
    hits.modal(area);
    let popup = dialog_area(area, dialog);
    frame.render_widget(Clear, popup);
    let block = window(dialog.title.clone(), dialog.tone)
        .style(Style::default().bg(crate::tui::theme::dialog()))
        .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height == 0 {
        return popup;
    }
    let body = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    frame.render_widget(
        Paragraph::new(dialog.body.clone()).wrap(Wrap { trim: false }),
        body,
    );
    let buttons = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    draw_buttons(
        frame,
        buttons,
        &dialog.buttons,
        focus.get(dialog),
        dialog.tone,
        "←/→ choose · Enter press · Esc cancel",
        hits,
    );
    popup
}

/// The frame of a window: rounded corners and a bold title in the tone's color, on the panel
/// background.
pub(in crate::tui) fn window(title: impl Into<String>, tone: Tone) -> Block<'static> {
    let title = title.into();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(tone.color()))
        .style(Style::default().bg(crate::tui::theme::panel()));
    if title.trim().is_empty() {
        return block;
    }
    block.title(Span::styled(
        format!(" {} ", title.trim()),
        Style::default()
            .fg(tone.color())
            .add_modifier(Modifier::BOLD),
    ))
}

/// Draws `buttons` right-aligned in the one-row `area`, with `hint` on the left when it fits,
/// and records them for the mouse.
pub(in crate::tui) fn draw_buttons(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    buttons: &[Button],
    highlighted: usize,
    tone: Tone,
    hint: &str,
    hits: &Hits,
) {
    use unicode_width::UnicodeWidthStr as _;
    if area.height == 0 || buttons.is_empty() {
        return;
    }
    let texts = buttons.iter().map(Button::text).collect::<Vec<_>>();
    let gap = 2u16;
    let total =
        texts.iter().map(|text| text.width() as u16).sum::<u16>() + gap * (texts.len() as u16 - 1);
    let mut x = area.right().saturating_sub(total).max(area.x);
    let hint_width = hint.width() as u16;
    if !hint.is_empty() && hint_width + 2 <= x.saturating_sub(area.x) {
        frame.render_widget(
            Paragraph::new(Span::styled(hint.to_owned(), hint_style())),
            Rect::new(area.x, area.y, hint_width, 1),
        );
    }
    for (index, (button, text)) in buttons.iter().zip(texts).enumerate() {
        let width = (text.width() as u16).min(area.right().saturating_sub(x));
        if width == 0 {
            break;
        }
        let style = if index == highlighted {
            Style::default()
                .fg(Color::Black)
                .bg(tone.color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let cell = Rect::new(x, area.y, width, 1);
        frame.render_widget(Paragraph::new(Span::styled(text, style)), cell);
        hits.click(cell, Click::Key(button.key));
        x = x.saturating_add(width + gap);
    }
}

/// The style of the key hints at the bottom of windows.
pub(in crate::tui) fn hint_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[cfg(test)]
mod tests {
    use super::{Button, Dialog, DialogFocus, Routed, Tone, dialog_area, draw_dialog, route};
    use crate::tui::mouse::{Click, Hits};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn question(text: &str) -> Dialog<'static> {
        Dialog::confirm(
            "test",
            "Delete it?",
            Tone::Danger,
            vec![Line::from(text.to_owned())],
            "Delete",
            "Cancel",
        )
    }

    #[test]
    fn a_confirmation_is_wide_short_and_centered() {
        let screen = Rect::new(0, 0, 120, 40);
        let area = dialog_area(screen, &question("Delete this session?"));
        assert!(area.width >= 60, "wide: {area:?}");
        assert!(area.height <= 7, "short: {area:?}");
        assert_eq!(area.x * 2 + area.width, screen.width, "centered across");
        assert!(
            area.y.abs_diff(screen.height - area.y - area.height) <= 1,
            "centered down"
        );
    }

    #[test]
    fn the_height_follows_the_text() {
        let screen = Rect::new(0, 0, 100, 40);
        let short = dialog_area(screen, &question("One line."));
        let long = dialog_area(screen, &question(&"many words here ".repeat(30)));
        assert!(long.height > short.height, "{short:?} {long:?}");
        let tiny = dialog_area(Rect::new(0, 0, 30, 6), &question(&"word ".repeat(80)));
        assert!(
            tiny.width <= 30 && tiny.height <= 6,
            "fits the screen: {tiny:?}"
        );
    }

    #[test]
    fn arrows_and_tab_move_between_buttons_and_enter_presses_the_highlighted_one() {
        let dialog = question("Sure?");
        let mut focus = DialogFocus::default();
        assert_eq!(focus.get(&dialog), 1, "the safe button starts highlighted");
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::Left)),
            Routed::Moved
        );
        assert_eq!(focus.get(&dialog), 0);
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::Left)),
            Routed::Moved
        );
        assert_eq!(focus.get(&dialog), 0, "stops at the first button");
        assert_eq!(route(&dialog, &mut focus, key(KeyCode::Tab)), Routed::Moved);
        assert_eq!(focus.get(&dialog), 1);
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::BackTab)),
            Routed::Moved
        );
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::Enter)),
            Routed::Press(KeyCode::Char('y'))
        );
        assert_eq!(focus.get(&dialog), 1, "pressing resets the highlight");
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::Right)),
            Routed::Moved
        );
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::Enter)),
            Routed::Press(KeyCode::Char('n'))
        );
    }

    #[test]
    fn letters_press_their_button_and_esc_presses_the_safe_one() {
        let dialog = question("Sure?").default_button(0);
        let mut focus = DialogFocus::default();
        assert_eq!(focus.get(&dialog), 0);
        for (code, pressed) in [
            (KeyCode::Char('y'), 'y'),
            (KeyCode::Char('Y'), 'y'),
            (KeyCode::Char('n'), 'n'),
            (KeyCode::Char('N'), 'n'),
            (KeyCode::Esc, 'n'),
        ] {
            assert_eq!(
                route(&dialog, &mut focus, key(code)),
                Routed::Press(KeyCode::Char(pressed)),
                "{code:?}"
            );
        }
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::Char('q'))),
            Routed::Other
        );
        assert_eq!(
            route(&dialog, &mut focus, key(KeyCode::Down)),
            Routed::Other
        );
    }

    #[test]
    fn a_highlight_is_not_carried_over_to_another_dialog() {
        let first = question("first");
        let mut focus = DialogFocus::default();
        route(&first, &mut focus, key(KeyCode::Left));
        assert_eq!(focus.get(&first), 0);
        let mut other = question("second");
        other.id = "other";
        assert_eq!(focus.get(&other), 1, "starts on its own default");
        focus.clear();
        assert_eq!(focus.get(&first), 1);
    }

    #[test]
    fn the_dialog_draws_its_title_text_and_buttons_and_records_them_for_the_mouse() {
        let dialog = Dialog {
            buttons: vec![
                Button::new("Allow", 'y'),
                Button::new("Deny", 'n'),
                Button::new("Later", 'l'),
            ],
            ..question("Remove the thing?")
        };
        let hits = Hits::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        let mut drawn = Rect::default();
        terminal
            .draw(|frame| {
                drawn = draw_dialog(frame, frame.area(), &dialog, &DialogFocus::default(), &hits)
            })
            .expect("draw");
        let screen = crate::tui::mouse::testing::rows(&terminal).join("\n");
        assert!(screen.contains("Delete it?"), "{screen}");
        assert!(screen.contains("Remove the thing?"), "{screen}");
        let (allow_x, allow_y) = crate::tui::mouse::testing::find(&terminal, "Allow");
        let (deny_x, deny_y) = crate::tui::mouse::testing::find(&terminal, "Deny");
        assert_eq!(allow_y, deny_y, "one row of buttons");
        assert_eq!(allow_y, drawn.bottom() - 2, "at the bottom");
        assert!(allow_x < deny_x);
        let (later_x, _) = crate::tui::mouse::testing::find(&terminal, "Later");
        assert!(later_x + 12 >= drawn.right() - 3, "right-aligned");
        assert_eq!(
            hits.click_at(deny_x, deny_y),
            Some(Click::Key(KeyCode::Char('n')))
        );
        assert_eq!(
            hits.click_at(0, 0),
            None,
            "nothing under a dialog is clickable"
        );
    }
}
