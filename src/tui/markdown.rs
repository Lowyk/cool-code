//! Turning the assistant's Markdown into styled terminal lines.
//!
//! Headings, emphasis, inline code, code blocks, lists, quotes, links, rules and tables are
//! drawn with styles instead of being shown as raw characters. Text is not wrapped here: the
//! caller's paragraph wraps long lines.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const TEXT: Color = Color::White;
const DIM: Color = Color::Rgb(125, 135, 148);
const CODE_TEXT: Color = Color::Rgb(205, 214, 224);
/// How wide a horizontal rule is drawn.
const RULE_WIDTH: usize = 40;

#[derive(Default)]
struct Table {
    rows: Vec<Vec<Vec<Span<'static>>>>,
    header_rows: usize,
}

struct Renderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    /// One entry per open list: the next number, or `None` for a bullet list.
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    /// The marker for the first line of the list item being written.
    bullet: Option<String>,
    code_block: bool,
    link: Option<String>,
    table: Option<Table>,
    cell: Vec<Span<'static>>,
    in_table_head: bool,
    /// A blank line is wanted before the next block.
    gap: bool,
}

impl Renderer {
    fn new() -> Renderer {
        Renderer {
            lines: Vec::new(),
            current: Vec::new(),
            styles: vec![Style::default().fg(TEXT)],
            lists: Vec::new(),
            quote_depth: 0,
            bullet: None,
            code_block: false,
            link: None,
            table: None,
            cell: Vec::new(),
            in_table_head: false,
            gap: false,
        }
    }

    fn style(&self) -> Style {
        *self.styles.last().expect("a base style")
    }

    fn push_style(&mut self, change: impl FnOnce(Style) -> Style) {
        let next = change(self.style());
        self.styles.push(next);
    }

    fn text(&mut self, text: &str) {
        let style = self.style();
        self.span(Span::styled(text.to_owned(), style));
    }

    fn span(&mut self, span: Span<'static>) {
        if self.table.is_some() {
            self.cell.push(span);
        } else {
            self.current.push(span);
        }
    }

    /// The text that starts every line of the current block: quote bars, then list indentation.
    fn prefix(&mut self, first_line: bool) -> Vec<Span<'static>> {
        let mut prefix = Vec::new();
        for _ in 0..self.quote_depth {
            prefix.push(Span::styled("▎ ", Style::default().fg(DIM)));
        }
        let depth = self.lists.len();
        if depth > 0 {
            let indent = "  ".repeat(depth - 1);
            match (first_line, self.bullet.take()) {
                (true, Some(bullet)) => {
                    prefix.push(Span::styled(
                        format!("{indent}{bullet}"),
                        Style::default().fg(DIM),
                    ));
                }
                _ => prefix.push(Span::raw(format!("{indent}  "))),
            }
        }
        prefix
    }

    /// Ends the line being written.
    fn flush(&mut self) {
        let mut spans = self.prefix(true);
        spans.append(&mut self.current);
        self.lines.push(Line::from(spans));
    }

    /// Starts a block: a blank line before it when something came before.
    fn begin_block(&mut self) {
        if self.gap && !self.lines.is_empty() && self.lists.is_empty() {
            self.lines.push(Line::default());
        }
        self.gap = false;
    }

    fn end_block(&mut self) {
        if !self.current.is_empty() || self.bullet.is_some() {
            self.flush();
        }
        self.gap = true;
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.begin_block(),
            Tag::Heading { level, .. } => {
                self.begin_block();
                let style = match level {
                    HeadingLevel::H1 => Style::default()
                        .fg(crate::tui::theme::accent_bright())
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    HeadingLevel::H2 => Style::default()
                        .fg(crate::tui::theme::accent_bright())
                        .add_modifier(Modifier::BOLD),
                    _ => Style::default()
                        .fg(crate::tui::theme::accent_soft())
                        .add_modifier(Modifier::BOLD),
                };
                self.styles.push(style);
            }
            Tag::BlockQuote(_) => {
                self.begin_block();
                self.quote_depth += 1;
                self.push_style(|style| {
                    style
                        .fg(Color::Rgb(175, 185, 196))
                        .add_modifier(Modifier::ITALIC)
                });
            }
            Tag::CodeBlock(kind) => {
                self.begin_block();
                self.code_block = true;
                if let CodeBlockKind::Fenced(language) = kind {
                    let language = language.split_whitespace().next().unwrap_or("");
                    if !language.is_empty() {
                        let mut spans = self.prefix(false);
                        spans.push(Span::styled(
                            format!("╭ {language}"),
                            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
                        ));
                        self.lines.push(Line::from(spans));
                    }
                }
            }
            Tag::List(first) => {
                if self.lists.is_empty() {
                    self.begin_block();
                } else if !self.current.is_empty() {
                    // A nested list starts under the text of the item that holds it.
                    self.flush();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                let bullet = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let text = format!("{number}. ");
                        *number += 1;
                        text
                    }
                    Some(None) => {
                        if self.lists.len() % 2 == 1 {
                            "• ".to_owned()
                        } else {
                            "◦ ".to_owned()
                        }
                    }
                    None => "• ".to_owned(),
                };
                self.bullet = Some(bullet);
            }
            Tag::Emphasis => self.push_style(|style| style.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|style| style.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(|style| style.add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.push_style(|style| {
                    style
                        .fg(crate::tui::theme::accent())
                        .add_modifier(Modifier::UNDERLINED)
                });
            }
            Tag::Image { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.text("[image: ");
            }
            Tag::Table(_) => {
                self.begin_block();
                self.table = Some(Table::default());
            }
            Tag::TableHead => {
                // The header's cells come without a row of their own.
                self.in_table_head = true;
                if let Some(table) = self.table.as_mut() {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.cell.clear(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_block(),
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.end_block();
            }
            TagEnd::BlockQuote(_) => {
                self.styles.pop();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                self.code_block = false;
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.gap = true;
                }
            }
            TagEnd::Item => {
                // A tight item has no paragraph of its own to end its line.
                if !self.current.is_empty() || self.bullet.is_some() {
                    self.flush();
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some(url) = self.link.take() {
                    let shown: String = self
                        .current
                        .iter()
                        .chain(self.cell.iter())
                        .map(|span| span.content.as_ref())
                        .collect();
                    if !shown.ends_with(&url) && !url.is_empty() {
                        self.span(Span::styled(format!(" ({url})"), Style::default().fg(DIM)));
                    }
                }
            }
            TagEnd::Image => {
                let url = self.link.take().unwrap_or_default();
                self.text(&format!("]({url})"));
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.cell);
                if let Some(row) = self.table.as_mut().and_then(|table| table.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::TableHead => {
                self.in_table_head = false;
                if let Some(table) = self.table.as_mut() {
                    table.header_rows = table.rows.len();
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.write_table(table);
                }
                self.gap = true;
            }
            _ => {}
        }
    }

    fn write_table(&mut self, table: Table) {
        let width_of = |cell: &Vec<Span<'static>>| {
            cell.iter()
                .map(|span| span.content.as_ref().width())
                .sum::<usize>()
        };
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|column| {
                table
                    .rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(width_of)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let bar = Style::default().fg(DIM);
        for (index, row) in table.rows.iter().enumerate() {
            let mut spans = self.prefix(false);
            for (column, width) in widths.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::styled(" │ ", bar));
                }
                let cell = row.get(column).cloned().unwrap_or_default();
                let used = width_of(&cell);
                for mut span in cell {
                    if index < table.header_rows {
                        span.style = span.style.add_modifier(Modifier::BOLD);
                    }
                    spans.push(span);
                }
                if column + 1 < columns {
                    spans.push(Span::raw(" ".repeat(width - used)));
                }
            }
            self.lines.push(Line::from(spans));
            if index + 1 == table.header_rows {
                let mut rule = self.prefix(false);
                let line = widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                rule.push(Span::styled(line, bar));
                self.lines.push(Line::from(rule));
            }
        }
    }

    fn code_text(&mut self, text: &str) {
        let style = Style::default().fg(CODE_TEXT);
        let body = text.strip_suffix('\n').unwrap_or(text);
        for line in body.split('\n') {
            let mut spans = self.prefix(false);
            spans.push(Span::styled("│ ", Style::default().fg(DIM)));
            spans.push(Span::styled(line.to_owned(), style));
            self.lines.push(Line::from(spans));
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) if self.code_block => self.code_text(&text),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let style = Style::default()
                    .fg(crate::tui::theme::accent_bright())
                    .bg(crate::tui::theme::panel());
                self.span(Span::styled(format!(" {code} "), style));
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => {
                if self.table.is_none() {
                    self.flush();
                }
            }
            Event::Rule => {
                self.begin_block();
                let mut spans = self.prefix(false);
                spans.push(Span::styled(
                    "─".repeat(RULE_WIDTH),
                    Style::default().fg(DIM),
                ));
                self.lines.push(Line::from(spans));
                self.gap = true;
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "☑ " } else { "☐ " };
                self.span(Span::styled(mark, Style::default().fg(DIM)));
            }
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            _ => {}
        }
    }
}

/// The Markdown `text` as styled lines. Plain text comes out as plain lines.
pub(super) fn render(text: &str) -> Vec<Line<'static>> {
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer::new();
    for event in Parser::new_ext(text, options) {
        renderer.event(event);
    }
    if !renderer.current.is_empty() {
        renderer.flush();
    }
    while renderer
        .lines
        .last()
        .is_some_and(|line| line.spans.is_empty())
    {
        renderer.lines.pop();
    }
    renderer.lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    fn find<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Span<'static> {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains(text))
            .unwrap_or_else(|| panic!("no span with {text:?} in {:?}", plain(lines)))
    }

    #[test]
    fn plain_text_stays_plain_and_keeps_its_line_breaks_between_paragraphs() {
        let lines = render("Just some words here.\n\nA second paragraph.");
        assert_eq!(
            plain(&lines),
            ["Just some words here.", "", "A second paragraph."]
        );
        assert_eq!(render("").len(), 0);
    }

    #[test]
    fn emphasis_is_styled_and_the_marks_disappear() {
        let lines = render("a **bold** and *italic* and ~~gone~~ word");
        assert_eq!(plain(&lines), ["a bold and italic and gone word"]);
        assert!(
            find(&lines, "bold")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            find(&lines, "italic")
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            find(&lines, "gone")
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        assert!(
            !find(&lines, "a ")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn inline_code_has_its_own_look_without_the_backticks() {
        let lines = render("run `cargo test` now");
        let text = plain(&lines).join("");
        assert!(!text.contains('`'), "{text}");
        let code = find(&lines, "cargo test");
        assert_ne!(code.style.bg, None, "code sits on a background");
    }

    #[test]
    fn headings_are_bold_and_lose_their_hashes() {
        let lines = render("# Title\n\n## Part\n\ntext");
        assert_eq!(plain(&lines), ["Title", "", "Part", "", "text"]);
        assert!(
            find(&lines, "Title")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            find(&lines, "Title")
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(
            find(&lines, "Part")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn code_blocks_keep_every_line_exactly_and_name_their_language() {
        let lines =
            render("Before\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nAfter");
        let text = plain(&lines);
        assert_eq!(text[0], "Before");
        assert_eq!(text[2], "╭ rust");
        assert_eq!(text[3], "│ fn main() {");
        assert_eq!(text[4], "│     println!(\"hi\");", "indentation is kept");
        assert_eq!(text[5], "│ }");
        assert_eq!(text.last().unwrap(), "After");
        assert!(!text.iter().any(|line| line.contains("```")));
    }

    #[test]
    fn an_unfinished_code_block_while_streaming_is_still_a_code_block() {
        let lines = render("Here:\n\n```sh\ncargo build\ncargo te");
        let text = plain(&lines);
        assert!(text.contains(&"│ cargo build".to_owned()), "{text:?}");
        assert!(text.contains(&"│ cargo te".to_owned()), "{text:?}");
    }

    #[test]
    fn lists_are_bulleted_numbered_and_nested() {
        let lines = render("- one\n- two\n  - inner\n\n1. first\n2. second");
        assert_eq!(
            plain(&lines),
            ["• one", "• two", "  ◦ inner", "", "1. first", "2. second"]
        );
    }

    #[test]
    fn task_lists_show_checkboxes() {
        let lines = render("- [x] done\n- [ ] todo");
        assert_eq!(plain(&lines), ["• ☑ done", "• ☐ todo"]);
    }

    #[test]
    fn quotes_get_a_bar_and_links_show_their_address() {
        let lines =
            render("> wise words\n\nsee [the docs](https://example.com/docs) and <https://x.org>");
        let text = plain(&lines);
        assert_eq!(text[0], "▎ wise words");
        assert!(
            text[2].contains("the docs (https://example.com/docs)"),
            "{text:?}"
        );
        assert!(
            !text[2].contains("https://x.org (https://x.org)"),
            "an address that is its own text is not repeated: {text:?}"
        );
        assert!(
            find(&lines, "the docs")
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    #[test]
    fn rules_and_hard_breaks_work() {
        let lines = render("a\\\nb\n\n---\n\nc");
        let text = plain(&lines);
        assert_eq!(text[0], "a");
        assert_eq!(text[1], "b");
        assert!(text.iter().any(|line| line.starts_with("────")), "{text:?}");
    }

    #[test]
    fn tables_are_aligned_with_a_rule_under_the_header() {
        let lines = render("| Name | Qty |\n|---|---|\n| apple | 3 |\n| fig | 12 |");
        assert_eq!(
            plain(&lines),
            ["Name  │ Qty", "──────┼────", "apple │ 3", "fig   │ 12"]
        );
        assert!(
            find(&lines, "Name")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            !find(&lines, "apple")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn a_long_line_is_left_whole_for_the_paragraph_to_wrap() {
        let long = "word ".repeat(80);
        let lines = render(&long);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn text_that_only_looks_like_markdown_is_not_mangled() {
        let lines = render("snake_case_name and 2 * 3 * 4 and a_b");
        assert_eq!(plain(&lines), ["snake_case_name and 2 * 3 * 4 and a_b"]);
    }

    fn screen(app: &crate::tui::state::App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 34)).expect("terminal");
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

    fn app_with_answer(text: &str) -> crate::tui::state::App {
        use crate::tui::state::{App, TranscriptEntry, TranscriptKind};
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: "what changed?".to_owned(),
        });
        app.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Assistant,
            text: text.to_owned(),
        });
        app
    }

    #[test]
    fn a_finished_answer_is_drawn_with_its_markdown_applied() {
        let app = app_with_answer(
            "## Result\n\nIt is **done** and `ok`.\n\n- first\n- second\n\n```sh\ncargo test\n```",
        );
        let shown = screen(&app);
        assert!(
            shown.contains("Result") && !shown.contains("## Result"),
            "{shown}"
        );
        assert!(shown.contains("It is done and  ok ."), "{shown}");
        assert!(!shown.contains("**") && !shown.contains('`'), "{shown}");
        assert!(
            shown.contains("• first") && shown.contains("• second"),
            "{shown}"
        );
        assert!(
            shown.contains("╭ sh") && shown.contains("│ cargo test"),
            "{shown}"
        );
        // What the user typed is never reinterpreted.
        let mut typed = app_with_answer("ok");
        typed.transcript[0].text = "keep **these** stars".to_owned();
        assert!(screen(&typed).contains("keep **these** stars"));
    }

    #[test]
    fn an_answer_still_streaming_shows_finished_lines_as_markdown_and_the_open_line_plainly() {
        use crate::tui::state::StreamingTurn;
        let mut app = app_with_answer("earlier");
        let mut turn = StreamingTurn::new(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )));
        turn.text = "# Plan\n\n- **step** one\n\n```sh\ncargo bu".to_owned();
        app.streaming = Some(turn);
        let shown = screen(&app);
        assert!(
            shown.contains("Plan") && !shown.contains("# Plan"),
            "{shown}"
        );
        assert!(
            shown.contains("• step one") && !shown.contains("**step**"),
            "{shown}"
        );
        assert!(
            shown.contains("cargo bu"),
            "the line being written is already visible: {shown}"
        );
    }

    #[test]
    fn a_long_conversation_renders_quickly_enough_to_redraw_every_frame() {
        let paragraph =
            "Some **bold** words, `code`, and a [link](https://example.com).\n\n".repeat(30);
        let text = format!("{paragraph}```rust\nfn main() {{}}\n```\n");
        let start = std::time::Instant::now();
        for _ in 0..200 {
            std::hint::black_box(render(&text));
        }
        let each = start.elapsed() / 200;
        // 200 answers of this size (about 2 KB each) must cost far less than a frame (40 ms).
        assert!(
            each * 200 < std::time::Duration::from_secs(1),
            "{each:?} per answer"
        );
    }
}
