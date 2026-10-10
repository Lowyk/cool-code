//! Editing the prompt: a cursor that moves by character (grapheme cluster), word and line, and
//! the keys that move it or delete around it.
//!
//! The cursor is kept as the number of bytes between it and the end of the input. Code that
//! replaces the whole input therefore leaves the cursor at the end, and inserting text before the
//! cursor never moves it relative to the text after it.

use crate::tui::state::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation as _;

/// What a key does to the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum EditKey {
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    Backspace,
    Delete,
    /// Delete back to the start of the word (Ctrl+Backspace, Alt+Backspace, Ctrl+H).
    DeleteWord,
    /// Delete back to the previous whitespace (Ctrl+W, as in a shell).
    DeleteToWhitespace,
}

/// The editing action for `key`, if it is one.
///
/// Terminals report Ctrl+Backspace in different ways: Windows and terminals with the enhanced
/// keyboard protocol send Backspace with Control, many Unix terminals send the Ctrl+H control
/// code instead, and some send a plain Backspace that cannot be told apart. Alt+Backspace
/// arrives as Backspace with Alt. Option+Left/Right on macOS terminals often arrive as Alt+B and
/// Alt+F. Control+Alt is AltGr on Windows, which types characters, so it is never a shortcut here.
pub(in crate::tui) fn edit_key(key: KeyEvent) -> Option<EditKey> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let only_control = control && !alt;
    let only_alt = alt && !control;
    Some(match key.code {
        KeyCode::Left if control || alt => EditKey::WordLeft,
        KeyCode::Left => EditKey::Left,
        KeyCode::Right if control || alt => EditKey::WordRight,
        KeyCode::Right => EditKey::Right,
        KeyCode::Home => EditKey::LineStart,
        KeyCode::End => EditKey::LineEnd,
        KeyCode::Char('a') if only_control => EditKey::LineStart,
        KeyCode::Char('e') if only_control => EditKey::LineEnd,
        KeyCode::Char('b') if only_alt => EditKey::WordLeft,
        KeyCode::Char('f') if only_alt => EditKey::WordRight,
        KeyCode::Backspace if control || alt => EditKey::DeleteWord,
        KeyCode::Backspace => EditKey::Backspace,
        KeyCode::Char('h') if only_control => EditKey::DeleteWord,
        KeyCode::Char('w') if only_control => EditKey::DeleteToWhitespace,
        KeyCode::Delete => EditKey::Delete,
        _ => return None,
    })
}

pub(in crate::tui) fn previous_boundary(text: &str, position: usize) -> usize {
    text[..position]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(index, _)| index)
}

pub(in crate::tui) fn next_boundary(text: &str, position: usize) -> usize {
    text[position..]
        .graphemes(true)
        .next()
        .map_or(text.len(), |cluster| position + cluster.len())
}

fn is_word(cluster: &str) -> bool {
    cluster
        .chars()
        .next()
        .is_some_and(|first| first.is_alphanumeric() || first == '_')
}

fn is_space(cluster: &str) -> bool {
    cluster.chars().all(char::is_whitespace)
}

/// Moves back over clusters that are not `inside`, then over clusters that are.
fn back_over(text: &str, position: usize, inside: impl Fn(&str) -> bool) -> usize {
    let mut clusters = text[..position].grapheme_indices(true).rev().peekable();
    let mut start = position;
    while let Some((index, _)) = clusters.next_if(|(_, cluster)| !inside(cluster)) {
        start = index;
    }
    while let Some((index, _)) = clusters.next_if(|(_, cluster)| inside(cluster)) {
        start = index;
    }
    start
}

/// The start of the word before the cursor (letters, digits and underscores make a word).
pub(in crate::tui) fn word_left(text: &str, position: usize) -> usize {
    back_over(text, position, is_word)
}

/// The end of the next word after the cursor.
pub(in crate::tui) fn word_right(text: &str, position: usize) -> usize {
    let mut clusters = text[position..].graphemes(true).peekable();
    let mut end = position;
    while let Some(cluster) = clusters.next_if(|cluster| !is_word(cluster)) {
        end += cluster.len();
    }
    while let Some(cluster) = clusters.next_if(|cluster| is_word(cluster)) {
        end += cluster.len();
    }
    end
}

/// Back to just after the previous whitespace, skipping whitespace right before the cursor.
pub(in crate::tui) fn whitespace_word_left(text: &str, position: usize) -> usize {
    back_over(text, position, |cluster| !is_space(cluster))
}

pub(in crate::tui) fn line_start(text: &str, position: usize) -> usize {
    text[..position].rfind('\n').map_or(0, |index| index + 1)
}

pub(in crate::tui) fn line_end(text: &str, position: usize) -> usize {
    text[position..]
        .find('\n')
        .map_or(text.len(), |index| position + index)
}

/// Applies `edit` to `text` with the cursor at `cursor`; returns the new cursor.
pub(in crate::tui) fn apply(text: &mut String, cursor: usize, edit: EditKey) -> usize {
    let delete_back_to = |text: &mut String, start: usize| {
        text.replace_range(start..cursor, "");
        start
    };
    match edit {
        EditKey::Left => previous_boundary(text, cursor),
        EditKey::Right => next_boundary(text, cursor),
        EditKey::WordLeft => word_left(text, cursor),
        EditKey::WordRight => word_right(text, cursor),
        EditKey::LineStart => line_start(text, cursor),
        EditKey::LineEnd => line_end(text, cursor),
        EditKey::Backspace => delete_back_to(text, previous_boundary(text, cursor)),
        EditKey::DeleteWord => delete_back_to(text, word_left(text, cursor)),
        EditKey::DeleteToWhitespace => delete_back_to(text, whitespace_word_left(text, cursor)),
        EditKey::Delete => {
            let end = next_boundary(text, cursor);
            text.replace_range(cursor..end, "");
            cursor
        }
    }
}

impl App {
    /// The cursor's byte position in the input, always on a character boundary.
    pub(in crate::tui) fn input_cursor(&self) -> usize {
        let mut position = self.input.len().saturating_sub(self.input_tail);
        while !self.input.is_char_boundary(position) {
            position -= 1;
        }
        position
    }

    pub(in crate::tui) fn set_input_cursor(&mut self, position: usize) {
        let mut position = position.min(self.input.len());
        while !self.input.is_char_boundary(position) {
            position -= 1;
        }
        self.input_tail = self.input.len() - position;
    }

    /// Replaces the whole input and puts the cursor at its end.
    pub(in crate::tui) fn set_input(&mut self, text: impl Into<String>) {
        self.input = text.into();
        self.input_tail = 0;
    }

    /// Types `text` at the cursor.
    pub(in crate::tui) fn insert_input(&mut self, text: &str) {
        let position = self.input_cursor();
        self.input.insert_str(position, text);
        self.input_tail = self.input.len() - position - text.len();
    }

    pub(in crate::tui) fn edit_input(&mut self, edit: EditKey) {
        let cursor = self.input_cursor();
        let next = apply(&mut self.input, cursor, edit);
        self.set_input_cursor(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// Runs `edits` on `text` starting with the cursor where `|` is, and shows the result the
    /// same way.
    fn after(text: &str, edits: &[EditKey]) -> String {
        let cursor = text.find('|').expect("a cursor mark");
        let mut text = text.replacen('|', "", 1);
        let mut cursor = cursor;
        for edit in edits {
            cursor = apply(&mut text, cursor, *edit);
            assert!(text.is_char_boundary(cursor), "{text:?} at {cursor}");
        }
        text.insert(cursor, '|');
        text
    }

    #[test]
    fn left_and_right_step_over_whole_characters_and_grapheme_clusters() {
        assert_eq!(after("ab|c", &[EditKey::Left]), "a|bc");
        assert_eq!(after("ab|c", &[EditKey::Right]), "abc|");
        assert_eq!(
            after("|abc", &[EditKey::Left]),
            "|abc",
            "stops at the start"
        );
        assert_eq!(after("abc|", &[EditKey::Right]), "abc|", "stops at the end");
        assert_eq!(after("ж|ё", &[EditKey::Right]), "жё|", "two-byte letters");
        assert_eq!(after("a界|", &[EditKey::Left]), "a|界", "a wide character");
        // An emoji with a skin tone and a family emoji joined with zero-width joiners are one
        // step each; so is a letter with a combining accent.
        assert_eq!(after("x👍🏽|", &[EditKey::Left]), "x|👍🏽");
        assert_eq!(after("|👨‍👩‍👧y", &[EditKey::Right]), "👨‍👩‍👧|y");
        assert_eq!(after("e\u{301}|", &[EditKey::Left]), "|e\u{301}");
    }

    #[test]
    fn backspace_and_delete_remove_one_character_around_the_cursor() {
        assert_eq!(after("ab|c", &[EditKey::Backspace]), "a|c");
        assert_eq!(after("ab|c", &[EditKey::Delete]), "ab|");
        assert_eq!(after("|abc", &[EditKey::Backspace]), "|abc");
        assert_eq!(after("abc|", &[EditKey::Delete]), "abc|");
        assert_eq!(after("привет|", &[EditKey::Backspace]), "приве|");
        assert_eq!(after("при|вет", &[EditKey::Delete]), "при|ет");
        assert_eq!(
            after("ok👍🏽|", &[EditKey::Backspace]),
            "ok|",
            "the whole cluster"
        );
        assert_eq!(after("|👨‍👩‍👧!", &[EditKey::Delete]), "|!");
        assert_eq!(after("a\n|b", &[EditKey::Backspace]), "a|b", "joins lines");
    }

    #[test]
    fn word_moves_skip_punctuation_and_spaces_like_a_shell() {
        assert_eq!(after("fix the bug|", &[EditKey::WordLeft]), "fix the |bug");
        assert_eq!(
            after("fix the bug|", &[EditKey::WordLeft, EditKey::WordLeft]),
            "fix |the bug"
        );
        assert_eq!(after("fix the |bug", &[EditKey::WordLeft]), "fix |the bug");
        assert_eq!(after("|fix the bug", &[EditKey::WordRight]), "fix| the bug");
        assert_eq!(after("fix| the bug", &[EditKey::WordRight]), "fix the| bug");
        assert_eq!(
            after("src/tui/mod.rs|", &[EditKey::WordLeft]),
            "src/tui/mod.|rs"
        );
        assert_eq!(after("слово ещё|", &[EditKey::WordLeft]), "слово |ещё");
        assert_eq!(after("|слово ещё", &[EditKey::WordRight]), "слово| ещё");
        assert_eq!(after("a  |", &[EditKey::WordLeft]), "|a  ");
        assert_eq!(after("|  ", &[EditKey::WordRight]), "  |");
    }

    #[test]
    fn deleting_words_backwards() {
        assert_eq!(after("fix the bug|", &[EditKey::DeleteWord]), "fix the |");
        assert_eq!(after("fix the |bug", &[EditKey::DeleteWord]), "fix |bug");
        assert_eq!(
            after("see src/main.rs|", &[EditKey::DeleteWord]),
            "see src/main.|"
        );
        assert_eq!(
            after("see src/main.rs|", &[EditKey::DeleteToWhitespace]),
            "see |",
            "Ctrl+W deletes back to the space"
        );
        assert_eq!(after("see  |", &[EditKey::DeleteToWhitespace]), "|");
        assert_eq!(after("ещё слово|", &[EditKey::DeleteWord]), "ещё |");
        assert_eq!(after("|abc", &[EditKey::DeleteWord]), "|abc");
    }

    #[test]
    fn home_and_end_go_to_the_start_and_end_of_the_current_line() {
        assert_eq!(
            after("one\ntw|o\nthree", &[EditKey::LineStart]),
            "one\n|two\nthree"
        );
        assert_eq!(
            after("one\ntw|o\nthree", &[EditKey::LineEnd]),
            "one\ntwo|\nthree"
        );
        assert_eq!(after("on|e", &[EditKey::LineStart]), "|one");
        assert_eq!(after("on|e", &[EditKey::LineEnd]), "one|");
        assert_eq!(after("one\n|", &[EditKey::LineStart]), "one\n|");
        assert_eq!(after("ж|ж\nж", &[EditKey::LineEnd]), "жж|\nж");
    }

    #[test]
    fn keys_map_to_edits_including_the_terminal_variants() {
        let none = KeyModifiers::NONE;
        let control = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;
        let cases = [
            (key(KeyCode::Left, none), Some(EditKey::Left)),
            (key(KeyCode::Right, none), Some(EditKey::Right)),
            (key(KeyCode::Left, control), Some(EditKey::WordLeft)),
            (key(KeyCode::Right, control), Some(EditKey::WordRight)),
            (key(KeyCode::Left, alt), Some(EditKey::WordLeft)),
            (key(KeyCode::Right, alt), Some(EditKey::WordRight)),
            (key(KeyCode::Char('b'), alt), Some(EditKey::WordLeft)),
            (key(KeyCode::Char('f'), alt), Some(EditKey::WordRight)),
            (key(KeyCode::Home, none), Some(EditKey::LineStart)),
            (key(KeyCode::End, none), Some(EditKey::LineEnd)),
            (key(KeyCode::Char('a'), control), Some(EditKey::LineStart)),
            (key(KeyCode::Char('e'), control), Some(EditKey::LineEnd)),
            (key(KeyCode::Backspace, none), Some(EditKey::Backspace)),
            (key(KeyCode::Delete, none), Some(EditKey::Delete)),
            (key(KeyCode::Backspace, control), Some(EditKey::DeleteWord)),
            (key(KeyCode::Backspace, alt), Some(EditKey::DeleteWord)),
            (key(KeyCode::Char('h'), control), Some(EditKey::DeleteWord)),
            (
                key(KeyCode::Char('w'), control),
                Some(EditKey::DeleteToWhitespace),
            ),
            (key(KeyCode::Char('b'), none), None),
            (key(KeyCode::Char('w'), none), None),
            (key(KeyCode::Up, none), None),
            (key(KeyCode::Char('c'), alt), None),
        ];
        for (pressed, expected) in cases {
            assert_eq!(edit_key(pressed), expected, "{pressed:?}");
        }
        // AltGr on Windows arrives as Control+Alt and types a character; it is not a word move.
        assert_eq!(edit_key(key(KeyCode::Char('b'), control | alt)), None);
    }

    fn app(text: &str) -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.set_input(text);
        app
    }

    #[test]
    fn typing_inserts_at_the_cursor_and_the_cursor_survives_a_replaced_input() {
        let mut app = app("hllo");
        assert_eq!(app.input_cursor(), 4, "starts at the end");
        app.set_input_cursor(1);
        app.insert_input("e");
        assert_eq!(app.input, "hello");
        assert_eq!(app.input_cursor(), 2);
        app.edit_input(EditKey::LineEnd);
        app.insert_input("ж");
        assert_eq!(app.input, "helloж");
        // Code that assigns the input directly leaves the cursor at the same distance from the
        // end, and a shorter text never puts it out of range or inside a character.
        app.set_input_cursor(0);
        app.input = "ж".to_owned();
        assert_eq!(app.input_cursor(), 0);
        app.input.clear();
        assert_eq!(app.input_cursor(), 0);
    }
}
