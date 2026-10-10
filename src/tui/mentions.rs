//! `@` file references: suggestions while typing, and confirming files outside the project.
//!
//! Typing `@` offers files from the project; a path with a separator browses that folder, and
//! `..`, `~` or an absolute path browse outside it when the user has allowed that in Settings →
//! Privacy. Each outside file is still confirmed before it is read.

use crate::tui::context::{Built, OutsideFile, OutsidePolicy, build_user_message_in, plain};
use crate::tui::dialog::{Dialog, Routed, Tone, draw_dialog, hint_style, route, window};
use crate::tui::mouse::{Click, Hits, Row as MouseRow, line_rect};
use crate::tui::state::App;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Suggestions shown at once.
/// Most matches kept for one query; the popup scrolls through them.
const MAX_SUGGESTIONS: usize = 200;
/// Rows of suggestions the popup shows at once.
const VISIBLE_SUGGESTIONS: usize = 8;
/// How long the list of project files is reused before it is read again.
const FILES_FRESH_FOR: Duration = Duration::from_secs(10);
const MAX_FILES: usize = 5_000;
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 30_000;
/// Entries read from one folder when browsing.
const MAX_BROWSED: usize = 800;
/// Toggling the outside-files setting this many times in a row, quickly, reveals the hidden
/// "don't ask" option.
pub(in crate::tui) const TOGGLES_TO_REVEAL: usize = 6;
const TOGGLE_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Suggestion {
    /// What goes after the `@`.
    pub(in crate::tui) insert: String,
    pub(in crate::tui) dir: bool,
    /// Shown but not selectable (an explanation instead of a file).
    pub(in crate::tui) disabled: bool,
}

pub(in crate::tui) struct MentionState {
    /// Where the `@` is in the input.
    start: usize,
    items: Vec<Suggestion>,
    selected: usize,
    /// The first row shown, so moving within the visible rows does not scroll.
    offset: usize,
}

/// A message waiting for the user to confirm files outside the project, one at a time.
pub(in crate::tui) struct OutsidePrompt {
    root: PathBuf,
    prompt: String,
    files: Vec<OutsideFile>,
    index: usize,
    approved: HashSet<PathBuf>,
}

/// The `@` word being typed at the end of `input` (the text before the cursor): where it starts
/// and what follows the `@`. `None` when the cursor is not inside a reference.
pub(in crate::tui) fn mention_query(input: &str) -> Option<(usize, String)> {
    let at = input
        .rmatch_indices('@')
        .map(|(index, _)| index)
        .find(|index| *index == 0 || input[..*index].ends_with(char::is_whitespace))?;
    let rest = &input[at + 1..];
    match rest.strip_prefix('"') {
        Some(quoted) if quoted.contains('"') => None,
        Some(quoted) => Some((at, quoted.to_owned())),
        None if rest.contains(char::is_whitespace) => None,
        None => Some((at, rest.to_owned())),
    }
}

fn ignored_folder(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | "vendor" | "dist" | "build" | ".venv" | "__pycache__"
    )
}

/// The project's files, as paths relative to `root`, skipping build output and anything that
/// looks like it holds secrets.
pub(in crate::tui) fn list_project_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut visited = 0;
    walk(root, root, 0, &mut files, &mut visited);
    files.sort_by_key(|path| (path.matches('/').count(), path.to_ascii_lowercase()));
    files
}

fn walk(root: &Path, folder: &Path, depth: usize, files: &mut Vec<String>, visited: &mut usize) {
    if depth > MAX_DEPTH || files.len() >= MAX_FILES || *visited >= MAX_ENTRIES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        *visited += 1;
        if *visited >= MAX_ENTRIES {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !ignored_folder(&name) {
                walk(root, &entry.path(), depth + 1, files, visited);
            }
        } else if kind.is_file()
            && !crate::guard::sensitive_path(&name)
            && let Ok(relative) = entry.path().strip_prefix(root)
        {
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// How well `path` fits what was typed: lower is better, `None` is no match.
fn rank(path: &str, query: &str) -> Option<u8> {
    let path = path.to_ascii_lowercase();
    let name = path.rsplit('/').next().unwrap_or(&path);
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else if path.contains(query) {
        Some(3)
    } else {
        // The letters in order, such as "mn" for "main".
        let mut remaining = query.chars();
        let mut wanted = remaining.next();
        for letter in path.chars() {
            if Some(letter) == wanted {
                wanted = remaining.next();
            }
        }
        wanted.is_none().then_some(4)
    }
}

fn disabled(text: &str) -> Suggestion {
    Suggestion {
        insert: text.to_owned(),
        dir: false,
        disabled: true,
    }
}

/// Whether `query` names a place rather than part of a file name.
fn is_path_like(query: &str) -> bool {
    let drive = query.len() >= 2
        && query.as_bytes()[1] == b':'
        && query.as_bytes()[0].is_ascii_alphabetic();
    query.contains('/')
        || query.contains('\\')
        || query.starts_with('~')
        || query == ".."
        || query == "."
        || drive
}

/// The folder a typed prefix such as `../src/` or `~/` points to.
fn resolve_folder(root: &Path, prefix: &str, home: Option<&Path>) -> PathBuf {
    let typed = if let Some(rest) = prefix.strip_prefix('~') {
        match home {
            Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
            None => PathBuf::from(prefix),
        }
    } else {
        PathBuf::from(prefix)
    };
    if typed.is_absolute() || typed.has_root() {
        typed
    } else {
        root.join(typed)
    }
}

/// What to offer for `query`. Browsing outside the project needs `outside_allowed`; without it
/// the only answer is an explanation.
pub(in crate::tui) fn suggestions(
    root: &Path,
    files: &[String],
    query: &str,
    outside_allowed: bool,
    home: Option<&Path>,
) -> Vec<Suggestion> {
    if query.is_empty() || is_path_like(query) {
        return browse(root, query, outside_allowed, home);
    }
    let lower = query.to_ascii_lowercase();
    let mut matches: Vec<(u8, &String)> = files
        .iter()
        .filter_map(|path| rank(path, &lower).map(|score| (score, path)))
        .collect();
    matches.sort_by_key(|(score, path)| (*score, path.len(), (*path).clone()));
    matches
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, path)| Suggestion {
            insert: path.clone(),
            dir: false,
            disabled: false,
        })
        .collect()
}

fn browse(root: &Path, query: &str, outside_allowed: bool, home: Option<&Path>) -> Vec<Suggestion> {
    // A bare `..`, `.`, `~` or drive letter becomes the folder it names.
    let single = match query {
        ".." => Some("../"),
        "." => Some("./"),
        "~" => Some("~/"),
        _ if query.len() == 2 && query.ends_with(':') => None,
        _ => None,
    };
    if let Some(folder) = single {
        let outside = folder != "./";
        if outside && !outside_allowed {
            return vec![disabled(OUTSIDE_HINT)];
        }
        return vec![Suggestion {
            insert: folder.to_owned(),
            dir: true,
            disabled: false,
        }];
    }
    let split = query.rfind(['/', '\\']).map_or(0, |index| index + 1);
    let (prefix, name_part) = query.split_at(split);
    let prefix = if query.len() == 2 && query.ends_with(':') {
        format!("{query}/")
    } else {
        prefix.replace('\\', "/")
    };
    let name_part = if query.len() == 2 && query.ends_with(':') {
        ""
    } else {
        name_part
    };
    let folder = resolve_folder(root, &prefix, home);
    let Ok(canonical) = folder.canonicalize() else {
        return Vec::new();
    };
    if !canonical.starts_with(root) && !outside_allowed {
        return vec![disabled(OUTSIDE_HINT)];
    }
    let Ok(entries) = std::fs::read_dir(&canonical) else {
        return Vec::new();
    };
    let wanted = name_part.to_ascii_lowercase();
    let mut found: Vec<(bool, String)> = entries
        .flatten()
        .take(MAX_BROWSED)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            let hidden = name.starts_with('.') && !wanted.starts_with('.');
            if hidden
                || ignored_folder(&name)
                || crate::guard::sensitive_path(&name)
                || !lower.starts_with(&wanted)
            {
                return None;
            }
            let dir = entry.file_type().ok()?.is_dir();
            Some((dir, name))
        })
        .collect();
    found.sort_by_key(|(dir, name)| (!*dir, name.to_ascii_lowercase()));
    found
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(dir, name)| Suggestion {
            insert: format!("{prefix}{name}{}", if dir { "/" } else { "" }),
            dir,
            disabled: false,
        })
        .collect()
}

/// Where the reference that starts at `start` ends, given the cursor inside it: the rest of the
/// word after the cursor, or up to the closing quote of a quoted name.
fn reference_end(input: &str, start: usize, cursor: usize) -> usize {
    let rest = &input[cursor..];
    if input[start + 1..].starts_with('"')
        && let Some(quote) = rest.find('"')
        && !rest[..quote].contains('\n')
    {
        return cursor + quote + 1;
    }
    cursor + rest.find(char::is_whitespace).unwrap_or(rest.len())
}

/// Moves a popup's highlight by `step`, wrapping around and skipping rows that `skip` rejects,
/// and scrolls just enough to keep it in view. Returns the new (selected, offset).
pub(in crate::tui) fn step_selection(
    selected: usize,
    offset: usize,
    count: usize,
    step: isize,
    skip: impl Fn(usize) -> bool,
) -> (usize, usize) {
    let mut chosen = selected;
    let mut next = selected as isize;
    for _ in 0..count {
        next = (next + step).rem_euclid(count as isize);
        if !skip(next as usize) {
            chosen = next as usize;
            break;
        }
    }
    let offset = if chosen < offset {
        chosen
    } else if chosen >= offset + VISIBLE_SUGGESTIONS {
        chosen + 1 - VISIBLE_SUGGESTIONS
    } else {
        offset
    };
    (chosen, offset)
}

const OUTSIDE_HINT: &str =
    "Outside the project: turn on \"Reference files outside the project\" in Settings → Privacy";

impl App {
    /// Recomputes the suggestions for the `@` word at the end of the input.
    pub(in crate::tui) fn refresh_mentions(&mut self) {
        let cursor = self.input_cursor();
        let Some((start, query)) = mention_query(&self.input[..cursor]) else {
            self.mention = None;
            self.mention_dismissed_at = None;
            return;
        };
        if self.mention_dismissed_at == Some(cursor) {
            self.mention = None;
            return;
        }
        self.mention_dismissed_at = None;
        let Ok(root) = std::env::current_dir().and_then(|path| path.canonicalize()) else {
            self.mention = None;
            return;
        };
        let stale = self
            .project_files
            .as_ref()
            .is_none_or(|(read_at, _)| read_at.elapsed() > FILES_FRESH_FOR);
        if stale && self.workspace_trusted {
            self.project_files = Some((Instant::now(), list_project_files(&root)));
        }
        let files = self
            .project_files
            .as_ref()
            .map(|(_, files)| files.as_slice())
            .unwrap_or_default();
        let items = if self.workspace_trusted {
            suggestions(
                &root,
                files,
                &query,
                self.settings.outside_files,
                dirs::home_dir().as_deref(),
            )
        } else {
            Vec::new()
        };
        let selected = self
            .mention
            .as_ref()
            .map_or(0, |state| state.selected)
            .min(items.len().saturating_sub(1));
        self.mention = (!items.is_empty()).then_some(MentionState {
            start,
            items,
            selected,
            offset: 0,
        });
    }

    pub(in crate::tui) fn mention_move(&mut self, step: isize) {
        if let Some(state) = self.mention.as_mut() {
            // Explanations cannot be chosen, so the highlight skips them.
            (state.selected, state.offset) = step_selection(
                state.selected,
                state.offset,
                state.items.len(),
                step,
                |index| state.items[index].disabled,
            );
        }
    }

    /// Whether Enter should take the highlighted suggestion instead of sending the message: it
    /// should unless what is typed already is that suggestion.
    pub(in crate::tui) fn mention_takes_enter(&self) -> bool {
        let Some(state) = self.mention.as_ref() else {
            return false;
        };
        let Some(item) = state
            .items
            .get(state.selected)
            .filter(|item| !item.disabled)
        else {
            return false;
        };
        mention_query(&self.input[..self.input_cursor()])
            .is_some_and(|(_, typed)| typed.replace('\\', "/") != item.insert)
    }

    pub(in crate::tui) fn accept_mention(&mut self) {
        let Some(state) = self.mention.take() else {
            return;
        };
        let Some(item) = state
            .items
            .get(state.selected)
            .filter(|item| !item.disabled)
        else {
            self.mention = Some(state);
            return;
        };
        let quoted = item.insert.contains(char::is_whitespace);
        let mut token = String::from("@");
        if quoted {
            token.push('"');
        }
        token.push_str(&item.insert);
        let end = reference_end(&self.input, state.start, self.input_cursor());
        let followed_by_space = self.input[end..].starts_with(char::is_whitespace);
        let mut cursor = state.start + token.len();
        if !item.dir {
            if quoted {
                token.push('"');
                cursor += 1;
            }
            if !followed_by_space {
                token.push(' ');
            }
            // The cursor goes past the space, whether it was added or already there.
            cursor += 1;
        }
        self.input.replace_range(state.start..end, &token);
        self.set_input_cursor(cursor);
        self.refresh_mentions();
    }

    pub(in crate::tui) fn dismiss_mention(&mut self) {
        self.mention = None;
        self.mention_dismissed_at = Some(self.input_cursor());
    }

    /// Notes that the outside-files setting was switched; six quick switches unlock the option
    /// to stop asking.
    pub(in crate::tui) fn record_outside_toggle(&mut self) {
        let now = Instant::now();
        self.outside_toggles
            .retain(|earlier| now.duration_since(*earlier) <= TOGGLE_WINDOW);
        self.outside_toggles.push(now);
        if self.outside_toggles.len() >= TOGGLES_TO_REVEAL && !self.outside_no_prompt_revealed {
            self.outside_no_prompt_revealed = true;
            self.notice = "A hidden option appeared below.".to_owned();
        }
    }

    /// Reads the `@` references in `prompt` and sends the message. A file outside the project
    /// is confirmed first, one file at a time; `approved` holds the ones already confirmed.
    pub(in crate::tui) fn send_prompt(
        &mut self,
        prompt: String,
        approved: &HashSet<PathBuf>,
    ) -> Result<()> {
        let root = std::env::current_dir()?.canonicalize()?;
        self.send_prompt_in(&root, prompt, approved)
    }

    /// [`send_prompt`] for the project at `root`.
    pub(in crate::tui) fn send_prompt_in(
        &mut self,
        root: &Path,
        prompt: String,
        approved: &HashSet<PathBuf>,
    ) -> Result<()> {
        let policy = OutsidePolicy {
            allowed: self.settings.outside_files,
            no_prompt: self.settings.outside_files_no_prompt,
            approved,
        };
        let user_message =
            match build_user_message_in(root, &prompt, self.workspace_trusted, &policy) {
                Ok(Built::Message(message)) => message,
                Ok(Built::NeedsApproval(files)) => {
                    self.outside_prompt = Some(OutsidePrompt {
                        root: root.to_path_buf(),
                        prompt,
                        files,
                        index: 0,
                        approved: approved.clone(),
                    });
                    return Ok(());
                }
                Err(error) => {
                    // Keep what was typed so it can be fixed.
                    self.input = prompt;
                    self.notice = format!("Could not attach reference: {error:#}");
                    return Ok(());
                }
            };
        self.send_built_message(user_message)
    }

    pub(in crate::tui) fn handle_outside_prompt_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(prompt) = self.outside_prompt.as_mut() else {
            return Ok(());
        };
        let pressed = match route(&outside_dialog(prompt), &mut self.dialog_focus, key) {
            Routed::Press(code) => code,
            Routed::Moved | Routed::Other => return Ok(()),
        };
        match pressed {
            KeyCode::Char('y') => {
                let file = prompt.files[prompt.index].path.clone();
                prompt.approved.insert(file);
                prompt.index += 1;
                if prompt.index >= prompt.files.len() {
                    let done = self.outside_prompt.take().expect("prompt is open");
                    self.send_prompt_in(&done.root, done.prompt, &done.approved)?;
                }
            }
            _ => {
                let cancelled = self.outside_prompt.take().expect("prompt is open");
                self.input = cancelled.prompt;
                self.notice =
                    "Nothing was sent: a file outside the project was not allowed.".to_owned();
            }
        }
        Ok(())
    }
}

/// The suggestion list, drawn just above the prompt.
pub(in crate::tui) fn draw_mentions(
    frame: &mut ratatui::Frame<'_>,
    prompt_area: Rect,
    state: &MentionState,
    hits: &Hits,
) {
    let rows = state
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            if item.disabled {
                return Line::from(Span::styled(
                    item.insert.clone(),
                    Style::default().fg(Color::Rgb(255, 197, 92)),
                ));
            }
            let current = index == state.selected;
            Line::from(vec![
                popup_marker(current),
                Span::styled(
                    item.insert.clone(),
                    if current {
                        popup_current()
                    } else if item.dir {
                        Style::default().fg(crate::tui::theme::accent_soft())
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
            ])
        })
        .collect();
    let choosable = state
        .items
        .iter()
        .map(|item| !item.disabled)
        .collect::<Vec<_>>();
    draw_list_popup(
        frame,
        prompt_area,
        hits,
        ListPopup {
            title: "Files",
            hint: " ↑/↓ choose · Tab or Enter to insert · Esc closes ",
            rows,
            choosable: &choosable,
            selected: state.selected,
            offset: state.offset,
        },
    );
}

/// The "› " in front of the highlighted row of a popup list.
pub(in crate::tui) fn popup_marker(current: bool) -> Span<'static> {
    Span::styled(
        if current { "› " } else { "  " },
        Style::default().fg(crate::tui::theme::accent()),
    )
}

/// The style of the highlighted row's main text.
pub(in crate::tui) fn popup_current() -> Style {
    Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

/// What a popup list above the prompt shows.
pub(in crate::tui) struct ListPopup<'a> {
    pub(in crate::tui) title: &'a str,
    pub(in crate::tui) hint: &'a str,
    pub(in crate::tui) rows: Vec<Line<'static>>,
    /// Which rows ↑/↓ can land on; the others are explanations and are not clickable.
    pub(in crate::tui) choosable: &'a [bool],
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) offset: usize,
}

/// A scrolling list drawn just above the prompt, with the position of the highlight at the
/// bottom. Used for `@` files and for `/` commands.
pub(in crate::tui) fn draw_list_popup(
    frame: &mut ratatui::Frame<'_>,
    prompt_area: Rect,
    hits: &Hits,
    list: ListPopup<'_>,
) {
    let ListPopup {
        title,
        hint,
        rows,
        choosable,
        selected,
        offset,
    } = list;
    let shown = rows.len().min(VISIBLE_SUGGESTIONS);
    let height = (shown as u16 + 2).min(prompt_area.y);
    if height < 3 {
        return;
    }
    let popup = Rect::new(
        prompt_area.x,
        prompt_area.y - height,
        prompt_area.width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = window(title, Tone::Normal)
        .title_bottom(Line::from(Span::styled(hint.to_owned(), hint_style())))
        .title_bottom(Line::from(format!(" {}/{} ", selected + 1, rows.len())).right_aligned());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let visible = inner.height as usize;
    // Keep the highlighted row inside the visible window, even if the popup is shorter than usual.
    let first = offset
        .max((selected + 1).saturating_sub(visible))
        .min(selected);
    // ↑/↓ skip the explanations, so a row's place counts only the choosable rows.
    let place = |end: usize| {
        choosable[..end.min(choosable.len())]
            .iter()
            .filter(|row| **row)
            .count()
    };
    let current = place(selected);
    hits.wheel_arrows(popup);
    for (row, index) in (first..rows.len().min(first + visible)).enumerate() {
        if choosable.get(index).copied().unwrap_or(false) {
            hits.click(
                line_rect(inner, row),
                Click::Row(MouseRow::new(place(index), current).activate(Some(KeyCode::Tab))),
            );
        }
    }
    let lines = rows
        .into_iter()
        .skip(first)
        .take(visible)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The question about a file outside the project. Allow starts highlighted, as Enter has always
/// allowed the file.
fn outside_dialog(prompt: &OutsidePrompt) -> Dialog<'static> {
    let mut dialog = Dialog::confirm(
        "outside",
        "A file outside the project",
        Tone::Warning,
        Vec::new(),
        "Allow this file",
        "Cancel",
    )
    .default_button(0);
    let Some(file) = prompt.files.get(prompt.index) else {
        return dialog;
    };
    if file.sensitive {
        dialog.tone = Tone::Danger;
    }
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from("Let the model read this file?"),
        Line::from(""),
        Line::from(Span::styled(
            plain(&file.path),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(format!("{} bytes", file.size), dim)),
        Line::from(""),
    ];
    if file.sensitive {
        lines.push(Line::from(Span::styled(
            "This path looks like it may hold secrets. Only allow it if you mean to send them.",
            Style::default().fg(Color::Rgb(235, 80, 80)),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "Its contents are sent to your model provider with the message.",
        dim,
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "Cancel sends nothing · file {} of {}",
            prompt.index + 1,
            prompt.files.len()
        ),
        dim,
    )));
    dialog.body = lines;
    dialog
}

pub(in crate::tui) fn draw_outside_prompt(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    prompt: &OutsidePrompt,
    focus: &crate::tui::dialog::DialogFocus,
    hits: &crate::tui::mouse::Hits,
) {
    draw_dialog(frame, area, &outside_dialog(prompt), focus, hits);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (PathBuf, PathBuf) {
        let outer = std::env::temp_dir().join(format!("harness-mentions-{}", uuid::Uuid::new_v4()));
        let project = outer.join("project");
        for folder in [
            "src/tui",
            "docs",
            "target/debug",
            "node_modules/x",
            ".git/objects",
        ] {
            std::fs::create_dir_all(project.join(folder)).unwrap();
        }
        for file in [
            "README.md",
            "Cargo.toml",
            "src/main.rs",
            "src/tui/mod.rs",
            "src/tui/markdown.rs",
            "docs/architecture.md",
            "target/debug/junk.o",
            "node_modules/x/index.js",
            ".git/objects/pack",
            ".env",
            "src/credentials.rs",
        ] {
            std::fs::write(project.join(file), "x").unwrap();
        }
        std::fs::write(outer.join("sibling.txt"), "x").unwrap();
        std::fs::write(outer.join("also.txt"), "x").unwrap();
        std::fs::create_dir_all(outer.join("shared")).unwrap();
        (
            project.canonicalize().unwrap(),
            outer.canonicalize().unwrap(),
        )
    }

    fn inserts(items: &[Suggestion]) -> Vec<&str> {
        items.iter().map(|item| item.insert.as_str()).collect()
    }

    #[test]
    fn the_word_being_typed_is_found_only_while_it_is_open() {
        assert_eq!(
            mention_query("explain @src/ma"),
            Some((8, "src/ma".to_owned()))
        );
        assert_eq!(mention_query("@"), Some((0, String::new())));
        assert_eq!(
            mention_query("explain @src/main.rs and"),
            None,
            "the word ended"
        );
        assert_eq!(
            mention_query("mail me@example.com"),
            None,
            "not at a word start"
        );
        assert_eq!(mention_query("see @\"my fi"), Some((4, "my fi".to_owned())));
        assert_eq!(mention_query("see @\"my file.txt\" now"), None);
        assert_eq!(mention_query("no reference here"), None);
        assert_eq!(mention_query("@a done @b"), Some((8, "b".to_owned())));
    }

    #[test]
    fn project_files_skip_build_output_secrets_and_git() {
        let (project, _) = tree();
        let files = list_project_files(&project);
        for expected in [
            "README.md",
            "Cargo.toml",
            "src/main.rs",
            "src/tui/mod.rs",
            "docs/architecture.md",
        ] {
            assert!(
                files.contains(&expected.to_owned()),
                "{expected}: {files:?}"
            );
        }
        for hidden in [
            "target/debug/junk.o",
            "node_modules/x/index.js",
            ".git/objects/pack",
            ".env",
            "src/credentials.rs",
        ] {
            assert!(
                !files.contains(&hidden.to_owned()),
                "{hidden} must not be offered"
            );
        }
        assert_eq!(files[0], "Cargo.toml", "shallow files come first");
    }

    #[test]
    fn typing_part_of_a_name_finds_files_best_matches_first() {
        let (project, _) = tree();
        let files = list_project_files(&project);
        let found = suggestions(&project, &files, "main", false, None);
        assert_eq!(found[0].insert, "src/main.rs");
        let found = suggestions(&project, &files, "mod", false, None);
        assert_eq!(inserts(&found), ["src/tui/mod.rs"]);
        let found = suggestions(&project, &files, "tui", false, None);
        assert!(
            inserts(&found).contains(&"src/tui/markdown.rs"),
            "{found:?}"
        );
        let found = suggestions(&project, &files, "mkd", false, None);
        assert_eq!(inserts(&found), ["src/tui/markdown.rs"], "letters in order");
        assert!(suggestions(&project, &files, "zzzz", false, None).is_empty());
        let found = suggestions(&project, &files, "READ", false, None);
        assert_eq!(found[0].insert, "README.md", "case does not matter");
    }

    #[test]
    fn a_bare_at_lists_the_project_folder_with_folders_first() {
        let (project, _) = tree();
        let found = suggestions(&project, &[], "", false, None);
        assert_eq!(
            inserts(&found),
            ["docs/", "src/", "Cargo.toml", "README.md"]
        );
        assert!(found[0].dir && !found[2].dir);
    }

    #[test]
    fn a_path_with_a_separator_browses_that_folder() {
        let (project, _) = tree();
        let found = suggestions(&project, &[], "src/", false, None);
        assert_eq!(inserts(&found), ["src/tui/", "src/main.rs"]);
        let found = suggestions(&project, &[], "src/t", false, None);
        assert_eq!(inserts(&found), ["src/tui/"]);
        let found = suggestions(&project, &[], "src\\tui\\m", false, None);
        assert_eq!(
            inserts(&found),
            ["src/tui/markdown.rs", "src/tui/mod.rs"],
            "either separator works"
        );
        assert!(suggestions(&project, &[], "nowhere/", false, None).is_empty());
    }

    #[test]
    fn going_outside_the_project_needs_the_setting_and_otherwise_explains() {
        let (project, outer) = tree();
        for query in ["../", "..", "../si", &format!("{}/", outer.display())] {
            let found = suggestions(&project, &[], query, false, None);
            assert_eq!(found.len(), 1, "{query}: {found:?}");
            assert!(
                found[0].disabled && found[0].insert.contains("Settings → Privacy"),
                "{query}"
            );
        }
        let found = suggestions(&project, &[], "..", true, None);
        assert_eq!(inserts(&found), ["../"]);
        let found = suggestions(&project, &[], "../", true, None);
        assert_eq!(
            inserts(&found),
            ["../project/", "../shared/", "../also.txt", "../sibling.txt"]
        );
        let found = suggestions(&project, &[], "../s", true, None);
        assert_eq!(inserts(&found), ["../shared/", "../sibling.txt"]);
        assert!(
            !suggestions(&project, &[], "../", true, None)
                .iter()
                .any(|item| item.insert.contains(".env")),
            "secret-looking files are not offered even when browsing outside"
        );
    }

    #[test]
    fn climbing_out_and_back_in_is_still_inside() {
        let (project, _) = tree();
        let found = suggestions(&project, &[], "../project/src/", false, None);
        assert_eq!(
            inserts(&found),
            ["../project/src/tui/", "../project/src/main.rs"]
        );
    }

    #[test]
    fn the_home_folder_is_browsed_with_a_tilde_once_allowed() {
        let (project, outer) = tree();
        assert!(suggestions(&project, &[], "~/", false, Some(&outer))[0].disabled);
        let found = suggestions(&project, &[], "~/", true, Some(&outer));
        assert!(inserts(&found).contains(&"~/sibling.txt"), "{found:?}");
        assert_eq!(
            inserts(&suggestions(&project, &[], "~", true, Some(&outer))),
            ["~/"]
        );
    }

    fn app_in(project: &Path) -> App {
        // The popup reads the process's current folder, so tests that use it hold this lock.
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = true;
        let _ = project;
        app
    }

    #[test]
    fn accepting_a_file_inserts_it_with_a_space_and_a_folder_keeps_browsing() {
        let (project, _) = tree();
        let mut app = app_in(&project);
        app.input = "explain @src/".to_owned();
        app.mention = Some(MentionState {
            start: 8,
            items: vec![
                Suggestion {
                    insert: "src/tui/".to_owned(),
                    dir: true,
                    disabled: false,
                },
                Suggestion {
                    insert: "src/main.rs".to_owned(),
                    dir: false,
                    disabled: false,
                },
            ],
            selected: 1,
            offset: 0,
        });
        app.accept_mention();
        assert_eq!(app.input, "explain @src/main.rs ");
        let mut spaced = app_in(&project);
        spaced.input = "see @my".to_owned();
        spaced.mention = Some(MentionState {
            start: 4,
            items: vec![Suggestion {
                insert: "my notes.txt".to_owned(),
                dir: false,
                disabled: false,
            }],
            selected: 0,
            offset: 0,
        });
        spaced.accept_mention();
        assert_eq!(
            spaced.input, "see @\"my notes.txt\" ",
            "names with spaces are quoted"
        );
    }

    #[test]
    fn a_disabled_explanation_cannot_be_chosen_and_the_selection_skips_it() {
        let (project, _) = tree();
        let mut app = app_in(&project);
        app.input = "@..".to_owned();
        app.mention = Some(MentionState {
            start: 0,
            items: vec![disabled("explanation")],
            selected: 0,
            offset: 0,
        });
        app.accept_mention();
        assert_eq!(app.input, "@..", "nothing inserted");
        assert!(app.mention.is_some(), "the popup stays");
        assert!(!app.mention_takes_enter());
        app.mention = Some(MentionState {
            start: 0,
            items: vec![
                Suggestion {
                    insert: "a".to_owned(),
                    dir: false,
                    disabled: false,
                },
                disabled("note"),
                Suggestion {
                    insert: "b".to_owned(),
                    dir: false,
                    disabled: false,
                },
            ],
            selected: 0,
            offset: 0,
        });
        app.mention_move(1);
        assert_eq!(app.mention.as_ref().unwrap().selected, 2, "skips the note");
        app.mention_move(1);
        assert_eq!(app.mention.as_ref().unwrap().selected, 0, "wraps around");
        app.mention_move(-1);
        assert_eq!(app.mention.as_ref().unwrap().selected, 2);
    }

    #[test]
    fn enter_sends_the_message_when_the_reference_is_already_complete() {
        let (project, _) = tree();
        let mut app = app_in(&project);
        app.input = "read @src/main.rs".to_owned();
        app.mention = Some(MentionState {
            start: 5,
            items: vec![Suggestion {
                insert: "src/main.rs".to_owned(),
                dir: false,
                disabled: false,
            }],
            selected: 0,
            offset: 0,
        });
        assert!(
            !app.mention_takes_enter(),
            "what is typed already is the suggestion"
        );
        app.input = "read @src/ma".to_owned();
        assert!(app.mention_takes_enter());
    }

    #[test]
    fn dismissing_the_popup_keeps_it_closed_until_the_text_changes() {
        let (project, _) = tree();
        let mut app = app_in(&project);
        app.input = "@src".to_owned();
        app.mention = Some(MentionState {
            start: 0,
            items: vec![Suggestion {
                insert: "src/".to_owned(),
                dir: true,
                disabled: false,
            }],
            selected: 0,
            offset: 0,
        });
        app.dismiss_mention();
        assert!(app.mention.is_none());
        app.refresh_mentions();
        assert!(app.mention.is_none(), "still dismissed for the same text");
    }

    #[test]
    fn six_quick_toggles_reveal_the_hidden_option_and_fewer_or_slow_ones_do_not() {
        let (project, _) = tree();
        let mut app = app_in(&project);
        for _ in 0..TOGGLES_TO_REVEAL - 1 {
            app.record_outside_toggle();
        }
        assert!(!app.outside_no_prompt_revealed);
        app.record_outside_toggle();
        assert!(app.outside_no_prompt_revealed);
        let mut slow = app_in(&project);
        let long_ago = Instant::now() - TOGGLE_WINDOW - Duration::from_secs(1);
        slow.outside_toggles = vec![long_ago; TOGGLES_TO_REVEAL - 1];
        slow.record_outside_toggle();
        assert!(!slow.outside_no_prompt_revealed, "old toggles do not count");
    }

    fn key(code: KeyCode) -> event::KeyEvent {
        event::KeyEvent::new(code, event::KeyModifiers::NONE)
    }

    fn screen(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 36)).expect("terminal");
        terminal
            .draw(|frame| crate::tui::render::draw(frame, app, 0))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_long_list_keeps_every_match_and_scrolls_to_follow_the_choice() {
        let files: Vec<String> = (0..40).map(|n| format!("src/file{n:02}.rs")).collect();
        let found = suggestions(Path::new("."), &files, "file", false, None);
        assert_eq!(
            found.len(),
            40,
            "all matches are offered, not just the first few"
        );
        let mut app = trusted_app();
        app.input = "see @file".to_owned();
        app.mention = Some(MentionState {
            start: 4,
            items: found,
            selected: 0,
            offset: 0,
        });
        let first = screen(&app);
        assert!(first.contains("src/file00.rs"), "{first}");
        assert!(first.contains("1/40"), "the position is shown: {first}");
        assert!(!first.contains("src/file20.rs"));
        for _ in 0..20 {
            app.mention_move(1);
        }
        let later = screen(&app);
        assert!(
            later.contains("src/file20.rs"),
            "the list scrolled: {later}"
        );
        assert!(!later.contains("src/file00.rs"), "{later}");
        assert!(later.contains("21/40"), "{later}");
        // Moving back up inside the window moves the highlight, not the list.
        app.mention_move(-1);
        let up = screen(&app);
        assert!(
            up.contains("src/file20.rs") && up.contains("src/file13.rs"),
            "{up}"
        );
        // Wrapping from the top goes to the last match, which must be visible too.
        for _ in 0..21 {
            app.mention_move(-1);
        }
        assert!(screen(&app).contains("src/file39.rs"));
    }

    fn trusted_app() -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = true;
        app
    }

    #[test]
    fn an_outside_file_waits_for_a_yes_and_then_the_message_is_sent_with_it() {
        let (project, outer) = tree();
        let mut app = trusted_app();
        app.settings.outside_files = true;
        app.send_prompt_in(
            &project,
            "summarize @../sibling.txt".to_owned(),
            &HashSet::new(),
        )
        .expect("send");
        assert!(
            app.pending.is_none(),
            "nothing is sent before the file is confirmed"
        );
        assert!(app.outside_prompt.is_some());
        let shown = screen(&app);
        assert!(shown.contains("A file outside the project"), "{shown}");
        assert!(
            shown.contains("sibling.txt") && shown.contains("Allow this file (y)"),
            "{shown}"
        );
        app.handle_outside_prompt_key(key(KeyCode::Char('y')))
            .expect("yes");
        assert!(app.outside_prompt.is_none());
        assert!(app.pending.is_some(), "the message went out");
        let sent = app.messages.last().expect("the message");
        assert!(
            sent.display
                .contains(&format!("[File: @{}]", plain(&outer.join("sibling.txt")))),
            "{}",
            sent.display
        );
        assert!(
            sent.content.as_str().unwrap().contains("x"),
            "its contents are attached"
        );
    }

    #[test]
    fn the_outside_file_question_is_a_shared_dialog_with_the_same_answers() {
        use crate::tui::mouse::testing::{click_text, has_button};
        let asking = || {
            let (project, _) = tree();
            let mut app = trusted_app();
            app.settings.outside_files = true;
            app.send_prompt_in(
                &project,
                "summarize @../sibling.txt".to_owned(),
                &HashSet::new(),
            )
            .expect("send");
            app
        };
        assert!(has_button(&asking(), "Allow this file"));
        assert!(has_button(&asking(), "Cancel"));
        for how in ["Enter", "click"] {
            let mut app = asking();
            match how {
                "Enter" => app.handle_outside_prompt_key(key(KeyCode::Enter)).unwrap(),
                _ => click_text(&mut app, "Allow this file"),
            }
            assert!(app.outside_prompt.is_none(), "{how}");
            assert!(app.pending.is_some(), "{how}: the message went out");
        }
        for how in ["Esc", "click"] {
            let mut app = asking();
            match how {
                "Esc" => app.handle_outside_prompt_key(key(KeyCode::Esc)).unwrap(),
                _ => click_text(&mut app, "[ Cancel"),
            }
            assert!(
                app.outside_prompt.is_none() && app.pending.is_none(),
                "{how}"
            );
            assert!(app.messages.is_empty(), "{how}: nothing was sent");
            assert_eq!(app.input, "summarize @../sibling.txt", "{how}");
        }
    }

    #[test]
    fn clicking_a_suggestion_picks_it_and_a_second_click_inserts_it() {
        use crate::tui::mouse::testing::click_text;
        let mut app = trusted_app();
        app.project_files = Some((
            Instant::now(),
            vec![
                "alpha_notes.txt".to_owned(),
                "bravo_notes.txt".to_owned(),
                "charlie_notes.txt".to_owned(),
            ],
        ));
        app.input = "@_notes".to_owned();
        app.refresh_mentions();
        assert!(app.mention.is_some(), "the suggestions are open");
        click_text(&mut app, "charlie_notes.txt");
        assert_eq!(app.input, "@_notes", "the first click only picks it");
        click_text(&mut app, "charlie_notes.txt");
        assert_eq!(app.input, "@charlie_notes.txt ");
        assert!(app.mention.is_none());
    }

    #[test]
    fn saying_no_sends_nothing_and_puts_the_text_back_to_edit() {
        let (project, _) = tree();
        let mut app = trusted_app();
        app.settings.outside_files = true;
        app.send_prompt_in(
            &project,
            "summarize @../sibling.txt".to_owned(),
            &HashSet::new(),
        )
        .expect("send");
        app.handle_outside_prompt_key(key(KeyCode::Char('n')))
            .expect("no");
        assert!(app.outside_prompt.is_none() && app.pending.is_none());
        assert!(app.messages.is_empty(), "nothing was sent");
        assert_eq!(app.input, "summarize @../sibling.txt");
        assert!(app.notice.contains("Nothing was sent"), "{}", app.notice);
    }

    #[test]
    fn several_outside_files_are_confirmed_one_at_a_time_and_one_no_cancels_everything() {
        let (project, _) = tree();
        let mut app = trusted_app();
        app.settings.outside_files = true;
        let prompt = "compare @../sibling.txt and @../also.txt";
        app.send_prompt_in(&project, prompt.to_owned(), &HashSet::new())
            .expect("send");
        assert!(screen(&app).contains("file 1 of 2"));
        app.handle_outside_prompt_key(key(KeyCode::Enter))
            .expect("first yes");
        assert!(app.outside_prompt.is_some() && app.pending.is_none());
        assert!(screen(&app).contains("file 2 of 2"));
        app.handle_outside_prompt_key(key(KeyCode::Esc))
            .expect("second no");
        assert!(app.messages.is_empty());
        assert_eq!(app.input, prompt);
    }

    #[test]
    fn an_outside_file_is_refused_while_the_setting_is_off_and_the_text_is_kept() {
        let (project, _) = tree();
        let mut app = trusted_app();
        app.send_prompt_in(&project, "read @../sibling.txt".to_owned(), &HashSet::new())
            .expect("send");
        assert!(app.outside_prompt.is_none() && app.pending.is_none());
        assert_eq!(
            app.input, "read @../sibling.txt",
            "what was typed is not lost"
        );
        assert!(app.notice.contains("Settings → Privacy"), "{}", app.notice);
    }

    #[test]
    fn typing_an_at_shows_files_and_tab_or_enter_inserts_the_highlighted_one() {
        let mut app = trusted_app();
        for character in "look at @Cargo".chars() {
            crate::tui::handle_key(
                &mut app,
                event::KeyEvent::new(KeyCode::Char(character), event::KeyModifiers::NONE),
            )
            .expect("type");
        }
        let shown = screen(&app);
        assert!(
            shown.contains("Files") && shown.contains("Cargo.toml"),
            "{shown}"
        );
        crate::tui::handle_key(&mut app, key(KeyCode::Tab)).expect("tab");
        assert!(app.input.starts_with("look at @Cargo."), "{}", app.input);
        assert!(
            app.input.ends_with(' '),
            "a file ends with a space: {:?}",
            app.input
        );
        assert!(app.mention.is_none());
        assert!(
            app.messages.is_empty(),
            "choosing a file did not send the message"
        );
    }

    fn file(insert: &str) -> Suggestion {
        Suggestion {
            insert: insert.to_owned(),
            dir: false,
            disabled: false,
        }
    }

    #[test]
    fn a_reference_in_the_middle_of_the_text_is_completed_where_the_cursor_is() {
        let mut app = trusted_app();
        app.set_input("explain @src/ma and more");
        app.set_input_cursor("explain @src/ma".len());
        app.mention = Some(MentionState {
            start: 8,
            items: vec![file("src/main.rs")],
            selected: 0,
            offset: 0,
        });
        assert!(app.mention_takes_enter());
        app.accept_mention();
        assert_eq!(
            app.input, "explain @src/main.rs and more",
            "no doubled space"
        );
        assert_eq!(app.input_cursor(), "explain @src/main.rs ".len());
        // The part of the word after the cursor is replaced too.
        app.set_input("see @src/main.rs now");
        app.set_input_cursor("see @src/m".len());
        app.mention = Some(MentionState {
            start: 4,
            items: vec![file("src/tui/mod.rs")],
            selected: 0,
            offset: 0,
        });
        app.accept_mention();
        assert_eq!(app.input, "see @src/tui/mod.rs now");
        // A quoted name is replaced up to its closing quote.
        app.set_input("see @\"my no\" now");
        app.set_input_cursor("see @\"my".len());
        app.mention = Some(MentionState {
            start: 4,
            items: vec![file("my notes.txt")],
            selected: 0,
            offset: 0,
        });
        app.accept_mention();
        assert_eq!(app.input, "see @\"my notes.txt\" now");
    }

    #[test]
    fn the_word_is_read_up_to_the_cursor_not_the_end_of_the_input() {
        let mut app = trusted_app();
        app.set_input("look at  please");
        app.set_input_cursor("look at ".len());
        for character in "@Cargo.t".chars() {
            crate::tui::handle_key(
                &mut app,
                event::KeyEvent::new(KeyCode::Char(character), event::KeyModifiers::NONE),
            )
            .expect("type");
        }
        assert_eq!(app.input, "look at @Cargo.t please");
        let shown = screen(&app);
        assert!(shown.contains("Cargo.toml"), "{shown}");
        crate::tui::handle_key(&mut app, key(KeyCode::Tab)).expect("tab");
        assert_eq!(app.input, "look at @Cargo.toml please");
        assert!(app.mention.is_none());
        // Moving the cursor out of the word closes the list; moving back opens it again.
        app.set_input("@Cargo and");
        app.set_input_cursor("@Cargo".len());
        app.refresh_mentions();
        assert!(app.mention.is_some());
        crate::tui::handle_key(&mut app, key(KeyCode::End)).expect("end");
        assert!(app.mention.is_none());
    }

    #[test]
    fn escape_closes_the_suggestions_without_leaving_the_program() {
        let mut app = trusted_app();
        app.input = "@Cargo".to_owned();
        app.refresh_mentions();
        assert!(app.mention.is_some());
        crate::tui::handle_key(&mut app, key(KeyCode::Esc)).expect("escape");
        assert!(
            app.mention.is_none() && app.running,
            "the program keeps running"
        );
        crate::tui::handle_key(&mut app, key(KeyCode::Char('x'))).expect("type");
        assert!(app.input.ends_with("Cargox"));
    }

    #[test]
    fn nothing_is_suggested_in_a_folder_that_is_not_trusted() {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = false;
        app.input = "@Cargo".to_owned();
        app.refresh_mentions();
        assert!(app.mention.is_none());
    }

    #[test]
    fn the_privacy_page_has_the_outside_setting_and_the_hidden_option_appears_after_six_quick_toggles()
     {
        use crate::tui::settings::Section;
        let mut app = trusted_app();
        app.open_settings(Section::Privacy);
        app.handle_settings_view_key(key(KeyCode::Right))
            .expect("focus");
        for _ in 0..3 {
            app.handle_settings_view_key(key(KeyCode::Down))
                .expect("down");
        }
        let before = screen(&app);
        assert!(before.contains("Outside files"), "{before}");
        assert!(
            !before.contains("Skip confirmation"),
            "hidden at first: {before}"
        );
        // Down past the last row stays on it while the option is hidden.
        app.handle_settings_view_key(key(KeyCode::Down))
            .expect("down");
        for _ in 0..TOGGLES_TO_REVEAL {
            app.handle_settings_view_key(key(KeyCode::Enter))
                .expect("toggle");
        }
        assert!(
            !app.settings.outside_files,
            "an even number of toggles ends where it started"
        );
        let after = screen(&app);
        assert!(after.contains("Skip confirmation"), "{after}");
        // The hidden option can be reached and switched on, which also allows outside files.
        app.handle_settings_view_key(key(KeyCode::Down))
            .expect("down");
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("enable");
        assert!(app.settings.outside_files_no_prompt && app.settings.outside_files);
        // Turning the main setting off turns the shortcut off with it.
        app.handle_settings_view_key(key(KeyCode::Up)).expect("up");
        app.handle_settings_view_key(key(KeyCode::Enter))
            .expect("disable");
        assert!(!app.settings.outside_files && !app.settings.outside_files_no_prompt);
    }
}
