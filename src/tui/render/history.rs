//! The conversation: what you said in a tinted block with an accent bar, the answers as plain
//! text with a dim footer, and the line being written at the end. Each entry is measured and
//! drawn on its own, so a block can be tinted across the whole width however its text wraps.

use super::{mode_span, streaming_lines};
use crate::tui::state::{App, TranscriptKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

/// Columns before the text of every entry: the bar of a message and the space after it.
const INSET: u16 = 2;
/// Columns left empty at the right edge.
const RIGHT_MARGIN: u16 = 1;

/// One entry, ready to draw.
struct Block {
    lines: Vec<Line<'static>>,
    /// Drawn on the input background with a bar and a blank row above and below.
    tinted: bool,
}

fn blocks(app: &App) -> Vec<Block> {
    let mut blocks = Vec::new();
    for entry in &app.transcript {
        blocks.push(match entry.kind {
            TranscriptKind::User => Block {
                lines: plain(&entry.text, Color::White),
                tinted: true,
            },
            TranscriptKind::Assistant => {
                let mut lines = crate::tui::markdown::render(&entry.text);
                if let Some(footer) = &entry.footer {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        mode_span(&footer.mode, false),
                        Span::styled(
                            format!(" · {}", footer.details),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                Block {
                    lines,
                    tinted: false,
                }
            }
            TranscriptKind::CommandOutput => Block {
                lines: plain(&entry.text, Color::Rgb(200, 205, 212)),
                tinted: false,
            },
            TranscriptKind::Error => Block {
                lines: entry
                    .text
                    .lines()
                    .map(|line| {
                        Line::from(Span::styled(
                            format!("| {line}"),
                            Style::default().fg(Color::Rgb(255, 145, 150)),
                        ))
                    })
                    .collect(),
                tinted: false,
            },
        });
    }
    if let Some(turn) = app.streaming.as_ref()
        && turn.tool.is_none()
    {
        blocks.push(Block {
            lines: streaming_lines(turn, app.settings.pulse),
            tinted: false,
        });
    }
    blocks
}

fn plain(text: &str, color: Color) -> Vec<Line<'static>> {
    let style = Style::default().fg(color);
    let mut lines = text
        .lines()
        .map(|line| Line::from(Span::styled(line.to_owned(), style)))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

/// Where a block sits in the whole conversation, in rows.
struct Placed<'a> {
    block: &'a Block,
    /// First row of the block (its top padding, when tinted).
    top: usize,
    /// Rows of text after wrapping.
    text_rows: usize,
    /// Blank rows above the text, and below it, inside the tint.
    padding: usize,
}

impl Placed<'_> {
    /// Rows of the block itself, then one blank row before the next entry.
    fn rows(&self) -> usize {
        self.padding * 2 + self.text_rows + 1
    }
}

/// Draws as much of the conversation as fits in `area`, the end of it at the bottom unless the
/// user scrolled up.
pub(super) fn draw_history(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    if area.width <= INSET + RIGHT_MARGIN || area.height == 0 {
        return;
    }
    let text_width = area.width - INSET - RIGHT_MARGIN;
    let blocks = blocks(app);
    let mut placed = Vec::with_capacity(blocks.len());
    let mut total = 0usize;
    for block in &blocks {
        let text_rows = Paragraph::new(block.lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(text_width);
        let entry = Placed {
            block,
            top: total,
            text_rows,
            padding: usize::from(block.tinted),
        };
        total += entry.rows();
        placed.push(entry);
    }
    let viewport = area.height as usize;
    let max_scroll = total.saturating_sub(viewport);
    let scroll = max_scroll.saturating_sub((app.history_scroll as usize).min(max_scroll));
    let (first, last) = (scroll, scroll + viewport);
    let buffer = frame.buffer_mut();
    for entry in &placed {
        let end = entry.top + entry.rows();
        if end <= first || entry.top >= last {
            continue;
        }
        if entry.block.tinted {
            // The tint and the bar cover the padding rows and every wrapped row of the text.
            let tinted_rows = entry.padding * 2 + entry.text_rows;
            for row in entry.top..entry.top + tinted_rows {
                if row < first || row >= last {
                    continue;
                }
                let y = area.y + (row - first) as u16;
                for x in area.x..area.x + area.width {
                    buffer[(x, y)].set_bg(crate::tui::theme::input());
                }
                buffer[(area.x, y)]
                    .set_symbol("▌")
                    .set_fg(crate::tui::theme::accent());
            }
        }
    }
    for entry in &placed {
        let text_top = entry.top + entry.padding;
        let text_end = text_top + entry.text_rows;
        if text_end <= first || text_top >= last {
            continue;
        }
        let visible_from = text_top.max(first);
        let visible_to = text_end.min(last);
        let rect = Rect::new(
            area.x + INSET,
            area.y + (visible_from - first) as u16,
            text_width,
            (visible_to - visible_from) as u16,
        );
        let skipped = (visible_from - text_top).min(u16::MAX as usize) as u16;
        frame.render_widget(
            Paragraph::new(entry.block.lines.clone())
                .wrap(Wrap { trim: false })
                .scroll((skipped, 0)),
            rect,
        );
    }
}
