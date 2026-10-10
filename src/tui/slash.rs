//! `/` commands: the one table that lists them (it drives both `/help` and the popup), the popup
//! that offers them while typing, and the suggestion for a mistyped command.

use crate::tui::mentions::{draw_list_popup, popup_current, popup_marker, step_selection};
use crate::tui::state::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// A built-in command: its name without the slash, what may follow it, and what it does.
pub(in crate::tui) struct BuiltIn {
    pub(in crate::tui) name: &'static str,
    pub(in crate::tui) usage: &'static str,
    pub(in crate::tui) summary: &'static str,
}

/// Every built-in command, in the order `/help` and the popup show them.
pub(in crate::tui) const COMMANDS: &[BuiltIn] = &[
    command("help", "", "List the commands"),
    command("settings", "", "Open Settings"),
    command("provider", "", "Open the providers in Settings"),
    command(
        "model",
        "[id]",
        "Choose a model, or pick one by id or series name",
    ),
    command(
        "forcemodel",
        "<id>",
        "Set a model on the default provider exactly as typed",
    ),
    command("effort", "[level]", "Choose the effort level"),
    command("mode", "[name]", "Choose the permission mode"),
    command("chain", "[id]", "Edit model chains, or switch to one"),
    command("usage", "", "Show every provider's remaining usage"),
    command("stats", "[clear]", "Show your own usage history"),
    command(
        "compact",
        "",
        "Condense the older conversation into a summary now",
    ),
    command(
        "undo",
        "",
        "Take back the file changes from the model's last turn",
    ),
    command("files", "", "List the project's files (trusted folders)"),
    command(
        "read",
        "<path>",
        "Show a file from the project (trusted folders)",
    ),
    command(
        "search",
        "<text>",
        "Search the project's files (trusted folders)",
    ),
    command("git status", "", "Show the Git status (trusted folders)"),
    command("init", "", "Create a starter COOL.md"),
    command(
        "claudemd",
        "[on|off]",
        "Load CLAUDE.md in this project or stop",
    ),
    command(
        "agentsmd",
        "[on|off]",
        "Load AGENTS.md in this project or stop",
    ),
    command(
        "privacy",
        "[add <value>|clear|revoke]",
        "Local redaction values and acknowledgements",
    ),
    command(
        "plugin",
        "[install <git-url or path>|list|remove <name>]",
        "Install, list or remove plugins",
    ),
    command("resume", "[all]", "Continue a saved session"),
    command("clear", "", "Start a new conversation"),
    command("quit", "", "Save the session and exit (also /exit)"),
];

const fn command(name: &'static str, usage: &'static str, summary: &'static str) -> BuiltIn {
    BuiltIn {
        name,
        usage,
        summary,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum CommandKind {
    BuiltIn,
    Skill,
    Plugin,
}

/// One entry of the popup and of `/help`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct SlashItem {
    pub(in crate::tui) name: String,
    pub(in crate::tui) usage: String,
    pub(in crate::tui) summary: String,
    pub(in crate::tui) kind: CommandKind,
}

/// The popup's state. It is open while it has items.
#[derive(Debug, Default)]
pub(in crate::tui) struct SlashPopup {
    pub(in crate::tui) items: Vec<SlashItem>,
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) offset: usize,
    /// What was typed after the slash when the list was made.
    query: String,
    /// The input the user closed the list for; it stays closed until the input changes.
    dismissed: Option<String>,
}

impl SlashPopup {
    pub(in crate::tui) fn is_open(&self) -> bool {
        !self.items.is_empty()
    }
}

pub(in crate::tui) fn builtin_items() -> Vec<SlashItem> {
    COMMANDS
        .iter()
        .map(|command| SlashItem {
            name: command.name.to_owned(),
            usage: command.usage.to_owned(),
            summary: command.summary.to_owned(),
            kind: CommandKind::BuiltIn,
        })
        .collect()
}

impl SlashItem {
    /// Whether something has to follow the name (`<...>`), as opposed to optional `[...]`.
    fn needs_argument(&self) -> bool {
        self.usage.starts_with('<')
    }
}

/// The commands that fit `query` (what follows the slash): names that start with it first, then
/// names that contain it.
pub(in crate::tui) fn matching(catalog: &[SlashItem], query: &str) -> Vec<SlashItem> {
    let query = query.to_lowercase();
    let searchable = !query.is_empty() && !query.contains(char::is_whitespace);
    let mut exact = Vec::new();
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    for item in catalog {
        let name = item.name.to_lowercase();
        if name == query {
            exact.push(item.clone());
        } else if name.starts_with(&query) {
            starts.push(item.clone());
        } else if searchable && name.contains(&query) {
            contains.push(item.clone());
        }
    }
    exact.extend(starts);
    exact.extend(contains);
    exact
}

/// The number of single-character edits (insert, delete, replace, swap two neighbours) between
/// `a` and `b`.
pub(in crate::tui) fn edit_distance(a: &str, b: &str) -> usize {
    let a = a.chars().collect::<Vec<_>>();
    let b = b.chars().collect::<Vec<_>>();
    // rows[i][j]: the distance between the first i letters of `a` and the first j of `b`.
    let mut rows = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let replace = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + replace);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(rows[i - 2][j - 2] + 1);
            }
            rows[i][j] = best;
        }
    }
    rows[a.len()][b.len()]
}

/// The commands closest to a mistyped `word`, best first (at most three).
pub(in crate::tui) fn closest(word: &str, catalog: &[SlashItem]) -> Vec<String> {
    let word = word.to_lowercase();
    let length = word.chars().count();
    let allowed = if length <= 3 { 1 } else { 2 };
    let mut found: Vec<(usize, usize, String)> = Vec::new();
    for item in catalog {
        let name = item.name.to_lowercase();
        // A command of two words ("git status") is compared by its first word.
        let first = name.split_whitespace().next().unwrap_or_default();
        let distance = if length >= 2 && name.starts_with(&word) {
            0
        } else {
            edit_distance(&word, first)
        };
        let length_gap = first.chars().count().abs_diff(length);
        if distance <= allowed && !found.iter().any(|(_, _, seen)| *seen == item.name) {
            found.push((distance, length_gap, item.name.clone()));
        }
    }
    found.sort();
    found.into_iter().take(3).map(|(_, _, name)| name).collect()
}

/// What to say about `/word` that is not a command.
pub(in crate::tui) fn unknown_notice(word: &str, catalog: &[SlashItem]) -> String {
    let names = closest(word, catalog)
        .into_iter()
        .map(|name| format!("/{name}"))
        .collect::<Vec<_>>();
    let suggestion = match names.as_slice() {
        [] => String::new(),
        [one] => format!(" Did you mean {one}?"),
        [rest @ .., last] => format!(" Did you mean {} or {last}?", rest.join(", ")),
    };
    format!("Unknown command /{word}.{suggestion} Type /help to see every command.")
}

/// The `/help` text: every command with what follows it and what it does.
pub(in crate::tui) fn help_text(catalog: &[SlashItem]) -> String {
    let label = |item: &SlashItem| {
        if item.usage.is_empty() {
            format!("/{}", item.name)
        } else {
            format!("/{} {}", item.name, item.usage)
        }
    };
    let width = catalog
        .iter()
        .map(|item| label(item).chars().count())
        .max()
        .unwrap_or(0)
        .min(36);
    let mut text = String::new();
    for (kind, heading) in [
        (CommandKind::BuiltIn, "Commands:"),
        (
            CommandKind::Skill,
            "Skills (run one with /name, adding anything you want to say):",
        ),
        (CommandKind::Plugin, "Plugin commands:"),
    ] {
        let lines = catalog
            .iter()
            .filter(|item| item.kind == kind)
            .map(|item| format!("  {:<width$}  {}", label(item), item.summary))
            .collect::<Vec<_>>();
        if lines.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(heading);
        for line in lines {
            text.push('\n');
            text.push_str(line.trim_end());
        }
    }
    text.push_str("\n\nAttach files with @path. Type / to pick a command from a list.");
    text
}

/// The one-line list of command names for the notice line.
pub(in crate::tui) fn help_summary(catalog: &[SlashItem]) -> String {
    let names = catalog
        .iter()
        .filter(|item| item.kind == CommandKind::BuiltIn)
        .map(|item| format!("/{}", item.name))
        .collect::<Vec<_>>();
    format!(
        "Commands: {}. Attach workspace files with @path.",
        names.join(", ")
    )
}

impl App {
    /// Everything that can follow a slash: the built-in commands, then skills and plugin
    /// commands.
    pub(in crate::tui) fn slash_catalog(&self) -> Vec<SlashItem> {
        builtin_items()
    }

    /// Recomputes both suggestion lists after the input or the cursor changed.
    pub(in crate::tui) fn refresh_popups(&mut self) {
        self.refresh_mentions();
        self.refresh_slash();
    }

    /// Recomputes the popup for the command being typed: only at the start of the input, while
    /// the cursor is still in the command's name.
    pub(in crate::tui) fn refresh_slash(&mut self) {
        let before = &self.input[..self.input_cursor()];
        let Some(query) = before
            .strip_prefix('/')
            .filter(|query| !query.contains('\n'))
            .map(str::to_owned)
        else {
            self.slash = SlashPopup::default();
            return;
        };
        if self.slash.dismissed.as_deref() == Some(self.input.as_str()) {
            self.slash.items.clear();
            return;
        }
        let items = matching(&self.slash_catalog(), &query);
        let (selected, offset) = if query == self.slash.query {
            (
                self.slash.selected.min(items.len().saturating_sub(1)),
                self.slash.offset,
            )
        } else {
            (0, 0)
        };
        self.slash = SlashPopup {
            items,
            selected,
            offset,
            query,
            dismissed: None,
        };
    }

    pub(in crate::tui) fn slash_move(&mut self, step: isize) {
        let popup = &mut self.slash;
        if popup.is_open() {
            (popup.selected, popup.offset) = step_selection(
                popup.selected,
                popup.offset,
                popup.items.len(),
                step,
                |_| false,
            );
        }
    }

    /// Whether Enter completes the highlighted command instead of running what is typed.
    pub(in crate::tui) fn slash_takes_enter(&self) -> bool {
        let typed = self.slash.query.to_lowercase();
        self.slash.is_open()
            && !self
                .slash
                .items
                .iter()
                .any(|item| item.name.to_lowercase() == typed)
    }

    /// Puts the highlighted command in place of the one being typed, followed by a space when
    /// it needs an argument, and closes the list.
    pub(in crate::tui) fn accept_slash(&mut self) {
        let Some(item) = self.slash.items.get(self.slash.selected).cloned() else {
            return;
        };
        let cursor = self.input_cursor();
        let rest = &self.input[cursor..];
        let end = cursor + rest.find(char::is_whitespace).unwrap_or(rest.len());
        let spaced = self.input[end..].starts_with(char::is_whitespace);
        let mut text = format!("/{}", item.name);
        let mut position = text.len();
        if item.needs_argument() {
            if !spaced {
                text.push(' ');
            }
            // The cursor goes past the space, whether it was added or already there.
            position += 1;
        }
        self.input.replace_range(..end, &text);
        self.set_input_cursor(position);
        self.dismiss_slash();
    }

    pub(in crate::tui) fn dismiss_slash(&mut self) {
        self.slash.items.clear();
        self.slash.dismissed = Some(self.input.clone());
    }
}

/// The command list, drawn just above the prompt like the `@` file list.
pub(in crate::tui) fn draw_slash(
    frame: &mut ratatui::Frame<'_>,
    prompt_area: Rect,
    popup: &SlashPopup,
) {
    let width = popup
        .items
        .iter()
        .map(|item| item.name.chars().count() + item.usage.chars().count() + 2)
        .max()
        .unwrap_or(0);
    let rows = popup
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let current = index == popup.selected;
            let label = format!("/{}", item.name);
            let usage = if item.usage.is_empty() {
                String::new()
            } else {
                format!(" {}", item.usage)
            };
            let padding = width.saturating_sub(label.chars().count() + usage.chars().count());
            let mut spans = vec![
                popup_marker(current),
                Span::styled(
                    label,
                    if current {
                        popup_current()
                    } else {
                        Style::default().fg(crate::tui::theme::accent_soft())
                    },
                ),
                Span::styled(usage, Style::default().fg(Color::DarkGray)),
                Span::raw(" ".repeat(padding + 1)),
                Span::styled(item.summary.clone(), Style::default().fg(Color::Gray)),
            ];
            if item.kind != CommandKind::BuiltIn {
                spans.push(Span::styled(
                    match item.kind {
                        CommandKind::Skill => "  skill",
                        _ => "  plugin",
                    },
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Line::from(spans)
        })
        .collect();
    draw_list_popup(
        frame,
        prompt_area,
        " Commands · ↑/↓ choose · Tab or Enter to complete · Esc closes ",
        rows,
        popup.selected,
        popup.offset,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        crate::tui::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
    }

    fn names(items: &[SlashItem]) -> Vec<&str> {
        items.iter().map(|item| item.name.as_str()).collect()
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
    fn the_table_has_every_command_once_with_a_description() {
        let mut seen = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!(
                seen.insert(command.name),
                "{} is listed twice",
                command.name
            );
            assert!(!command.summary.is_empty(), "{}", command.name);
            assert!(!command.name.starts_with('/'), "{}", command.name);
        }
        for expected in [
            "help",
            "settings",
            "model",
            "effort",
            "mode",
            "usage",
            "stats",
            "compact",
            "undo",
            "files",
            "read",
            "search",
            "git status",
            "init",
            "claudemd",
            "agentsmd",
            "privacy",
            "resume",
            "clear",
            "quit",
            "plugin",
        ] {
            assert!(seen.contains(expected), "{expected} is missing");
        }
    }

    #[test]
    fn names_that_start_with_the_text_come_first_then_names_that_contain_it() {
        let catalog = builtin_items();
        let found = matching(&catalog, "mo");
        assert_eq!(names(&found)[..2], ["model", "mode"]);
        assert!(
            names(&matching(&catalog, "ode")).contains(&"model"),
            "contains"
        );
        assert_eq!(
            names(&matching(&catalog, "MOD")),
            names(&matching(&catalog, "mod"))
        );
        assert_eq!(
            matching(&catalog, "").len(),
            catalog.len(),
            "a bare slash lists all"
        );
        assert_eq!(names(&matching(&catalog, "git s")), ["git status"]);
        assert!(
            matching(&catalog, "model gpt").is_empty(),
            "arguments close it"
        );
        assert!(matching(&catalog, "zzz").is_empty());
    }

    #[test]
    fn typing_a_slash_opens_the_list_and_tab_completes_the_highlighted_command() {
        let mut app = app();
        type_text(&mut app, "/se");
        assert!(app.slash.is_open());
        let shown = screen(&app);
        assert!(shown.contains("Commands"), "{shown}");
        assert!(
            shown.contains("/settings") && shown.contains("Open Settings"),
            "{shown}"
        );
        assert!(shown.contains("/search"), "{shown}");
        press(&mut app, KeyCode::Down);
        let second = app.slash.items[1].name.clone();
        press(&mut app, KeyCode::Tab);
        assert!(
            app.input.starts_with(&format!("/{second}")),
            "{}",
            app.input
        );
        assert_eq!(app.input_cursor(), app.input.len());
        assert!(app.transcript.is_empty(), "nothing ran");
    }

    #[test]
    fn enter_completes_a_partial_command_and_runs_a_complete_one() {
        let mut app = app();
        type_text(&mut app, "/hel");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.input, "/help");
        assert!(app.transcript.is_empty(), "completed, not run");
        press(&mut app, KeyCode::Enter);
        assert!(app.input.is_empty(), "now it ran");
        assert!(app.transcript[1].text.contains("/settings"));
    }

    #[test]
    fn a_command_that_needs_an_argument_is_completed_with_a_space() {
        let mut app = app();
        type_text(&mut app, "/forcem");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.input, "/forcemodel ");
        assert!(!app.slash.is_open(), "the list closes for the argument");
        let mut multi = self::app();
        type_text(&mut multi, "/git");
        press(&mut multi, KeyCode::Tab);
        assert_eq!(multi.input, "/git status");
    }

    #[test]
    fn escape_closes_the_list_until_the_text_changes_and_does_not_quit() {
        let mut app = app();
        type_text(&mut app, "/mo");
        press(&mut app, KeyCode::Esc);
        assert!(!app.slash.is_open() && app.running);
        app.refresh_slash();
        assert!(!app.slash.is_open(), "still closed for the same text");
        type_text(&mut app, "d");
        assert!(app.slash.is_open());
        // Only the start of the input counts: a slash later in a message is just text.
        let mut later = self::app();
        type_text(&mut later, "use a/b");
        assert!(!later.slash.is_open());
        let mut moved = self::app();
        type_text(&mut moved, "/model x");
        assert!(!moved.slash.is_open());
        press(&mut moved, KeyCode::Home);
        press(&mut moved, KeyCode::Right);
        press(&mut moved, KeyCode::Right);
        assert!(
            moved.slash.is_open(),
            "the cursor is back in the command name"
        );
    }

    #[test]
    fn help_and_the_popup_come_from_the_same_table() {
        let mut app = app();
        app.set_input("/help");
        app.submit().expect("help");
        let help = app.transcript[1].text.clone();
        type_text(&mut app, "/");
        let listed = app.slash.items.clone();
        assert_eq!(listed.len(), COMMANDS.len());
        for command in COMMANDS {
            assert!(
                help.contains(&format!("/{}", command.name)),
                "{}",
                command.name
            );
            assert!(help.contains(command.summary), "{}", command.summary);
            assert!(listed.iter().any(|item| item.name == command.name));
        }
        assert!(app.notice.contains("/stats"), "{}", app.notice);
    }

    #[test]
    fn edit_distance_counts_single_edits_and_swaps() {
        assert_eq!(edit_distance("help", "help"), 0);
        assert_eq!(edit_distance("hepl", "help"), 1, "a swap is one edit");
        assert_eq!(edit_distance("modle", "model"), 1);
        assert_eq!(edit_distance("setings", "settings"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("ёж", "еж"), 1, "letters, not bytes");
    }

    #[test]
    fn a_mistyped_command_suggests_the_closest_ones_and_points_at_help() {
        let catalog = builtin_items();
        assert_eq!(closest("hepl", &catalog), ["help"]);
        assert_eq!(closest("setings", &catalog), ["settings"]);
        assert_eq!(closest("sett", &catalog), ["settings"], "a prefix counts");
        assert!(closest("qwertyuiop", &catalog).is_empty());
        let two = closest("mod", &catalog);
        assert!(two.contains(&"mode".to_owned()) && two.contains(&"model".to_owned()));
        assert!(two.len() <= 3);
        let notice = unknown_notice("hepl", &catalog);
        assert!(
            notice.contains("/hepl") && notice.contains("/help?"),
            "{notice}"
        );
        let none = unknown_notice("qwertyuiop", &catalog);
        assert!(
            none.contains("/help") && !none.contains("Did you mean"),
            "{none}"
        );
        let mut app = app();
        app.set_input("/modle gpt");
        app.submit().expect("unknown");
        assert!(app.notice.contains("Did you mean /model"), "{}", app.notice);
        assert!(app.notice.contains("/help"), "{}", app.notice);
    }
}
