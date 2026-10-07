use crate::tui::state::App;
use crate::write_settings;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum StatsTab {
    Overview,
    Models,
}

pub(in crate::tui) struct StatsView {
    pub(in crate::tui) tab: StatsTab,
    pub(in crate::tui) range: crate::stats::Range,
    pub(in crate::tui) records: Vec<crate::stats::Record>,
    pub(in crate::tui) confirm_clear: bool,
    /// First visible body line. The drawing code clamps it to the content, so keys can overshoot.
    scroll: std::cell::Cell<u16>,
    // Summaries are recomputed only when the range, the data, or the minute changes.
    summary_cache: std::cell::RefCell<Option<(SummaryKey, crate::stats::Summary)>>,
    grid_cache: std::cell::RefCell<Option<(GridKey, crate::stats::HeatGrid)>>,
}

/// Cache keys: what a cached value was computed from.
type SummaryKey = (crate::stats::Range, usize, i64);
type GridKey = (usize, usize, i64);

const RANGES: [crate::stats::Range; 3] = [
    crate::stats::Range::All,
    crate::stats::Range::Days(30),
    crate::stats::Range::Days(7),
];

impl StatsView {
    pub(in crate::tui) fn open(records: Vec<crate::stats::Record>) -> StatsView {
        StatsView {
            tab: StatsTab::Overview,
            range: crate::stats::Range::All,
            records,
            confirm_clear: false,
            scroll: std::cell::Cell::new(0),
            summary_cache: std::cell::RefCell::new(None),
            grid_cache: std::cell::RefCell::new(None),
        }
    }

    fn scroll_by(&self, delta: i32) {
        let next = i32::from(self.scroll.get()).saturating_add(delta);
        self.scroll.set(next.clamp(0, i32::from(u16::MAX)) as u16);
    }

    fn summary(&self, now_ts: i64) -> crate::stats::Summary {
        let key = (self.range, self.records.len(), now_ts.div_euclid(60));
        if let Some((cached_key, summary)) = self.summary_cache.borrow().as_ref()
            && *cached_key == key
        {
            return summary.clone();
        }
        let summary = crate::stats::summarize(&self.records, self.range, &chrono::Local, now_ts);
        *self.summary_cache.borrow_mut() = Some((key, summary.clone()));
        summary
    }

    fn heat(&self, now_ts: i64, weeks: usize) -> crate::stats::HeatGrid {
        let key = (weeks, self.records.len(), now_ts.div_euclid(60));
        if let Some((cached_key, grid)) = self.grid_cache.borrow().as_ref()
            && *cached_key == key
        {
            return grid.clone();
        }
        let grid = crate::stats::heat_grid(&self.records, &chrono::Local, now_ts, weeks);
        *self.grid_cache.borrow_mut() = Some((key, grid.clone()));
        grid
    }
}

impl App {
    pub(in crate::tui) fn open_stats(&mut self, confirm_clear: bool) {
        let mut view = StatsView::open(crate::stats::load());
        view.confirm_clear = confirm_clear;
        self.stats_view = Some(view);
    }

    pub(in crate::tui) fn handle_stats_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(view) = self.stats_view.as_mut() else {
            return Ok(());
        };
        if view.confirm_clear {
            let confirmed = matches!(key.code, KeyCode::Char('y' | 'Y'));
            view.confirm_clear = false;
            if confirmed {
                // A file that cannot be deleted (locked, read-only) must not end the session.
                match crate::stats::clear() {
                    Ok(()) => {
                        view.records.clear();
                        self.notice = "Usage history deleted.".to_owned();
                    }
                    Err(error) => {
                        self.notice = format!("Could not delete the usage history: {error:#}");
                    }
                }
            }
            return Ok(());
        }
        let position = RANGES
            .iter()
            .position(|range| *range == view.range)
            .unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.stats_view = None,
            KeyCode::Tab => {
                view.tab = match view.tab {
                    StatsTab::Overview => StatsTab::Models,
                    StatsTab::Models => StatsTab::Overview,
                };
                view.scroll.set(0);
            }
            KeyCode::Right => {
                view.range = RANGES[(position + 1) % RANGES.len()];
                view.scroll.set(0);
            }
            KeyCode::Left => {
                view.range = RANGES[(position + RANGES.len() - 1) % RANGES.len()];
                view.scroll.set(0);
            }
            KeyCode::Up => view.scroll_by(-1),
            KeyCode::Down => view.scroll_by(1),
            KeyCode::PageUp => view.scroll_by(-8),
            KeyCode::PageDown => view.scroll_by(8),
            KeyCode::Home => view.scroll.set(0),
            KeyCode::End => view.scroll.set(u16::MAX),
            KeyCode::Char('c') => view.confirm_clear = true,
            KeyCode::Char('r') if !self.settings.stats_enabled => {
                self.settings.stats_enabled = true;
                self.settings.stats_prompt_answered = true;
                self.notice = match write_settings(&self.settings) {
                    Ok(()) => "Usage stats are now recorded on this computer.".to_owned(),
                    Err(error) => format!("Could not save the setting: {error:#}"),
                };
            }
            _ => {}
        }
        Ok(())
    }
}

const HEAT: [Color; 5] = [
    Color::Rgb(58, 66, 74),
    Color::Rgb(48, 96, 118),
    Color::Rgb(62, 140, 170),
    Color::Rgb(98, 200, 232),
    Color::Rgb(196, 246, 255),
];

fn range_label(range: crate::stats::Range) -> &'static str {
    match range {
        crate::stats::Range::All => "All time",
        crate::stats::Range::Days(30) => "Last 30 days",
        crate::stats::Range::Days(7) => "Last 7 days",
        crate::stats::Range::Days(_) => "Custom range",
    }
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!(
            "{}…",
            text.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

fn stat_line(pairs: &[(&str, String)], column_width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    for (label, value) in pairs {
        let used = label.chars().count().max(16) + value.chars().count();
        spans.push(Span::styled(
            format!("{label:<16}"),
            Style::default().fg(Color::Gray),
        ));
        spans.push(Span::styled(
            value.clone(),
            Style::default().fg(Color::White),
        ));
        spans.push(Span::raw(" ".repeat(column_width.saturating_sub(used))));
    }
    Line::from(spans)
}

/// Everything the Overview tab shows, as text lines (also used by tests).
pub(in crate::tui) fn overview_lines(
    view: &StatsView,
    width: u16,
    now_ts: i64,
) -> Vec<Line<'static>> {
    use crate::stats::{compact_tokens, format_duration, fun_fact};

    let summary = view.summary(now_ts);
    if view.records.is_empty() {
        return vec![Line::from(Span::styled(
            "No usage recorded yet. Stats appear here after your next request.",
            Style::default().fg(Color::Gray),
        ))];
    }
    if summary.requests == 0 {
        return vec![Line::from(Span::styled(
            "No usage in this range. Press → to widen it.",
            Style::default().fg(Color::Gray),
        ))];
    }
    let mut lines = Vec::new();

    let weeks = ((width as usize).saturating_sub(6) / 2).clamp(4, 26);
    let grid = view.heat(now_ts, weeks);
    for row in 0..7 {
        let label = match row {
            0 => "Mon",
            2 => "Wed",
            4 => "Fri",
            _ => "",
        };
        let mut spans = vec![Span::styled(
            format!("{label:<4}"),
            Style::default().fg(Color::DarkGray),
        )];
        for column in &grid.columns {
            spans.push(match column[row] {
                None => Span::raw("  "),
                Some(0) => Span::styled("· ", Style::default().fg(HEAT[0])),
                Some(level) => Span::styled("■ ", Style::default().fg(HEAT[level as usize])),
            });
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));

    let tilde = if summary.estimated { "~" } else { "" };
    let total = format!(
        "{tilde}{} (in {} · out {})",
        compact_tokens(summary.total_tokens()),
        compact_tokens(summary.total_input),
        compact_tokens(summary.total_output)
    );
    let requests = if summary.failed + summary.cancelled > 0 {
        format!(
            "{} ({} failed, {} cancelled)",
            summary.requests, summary.failed, summary.cancelled
        )
    } else {
        summary.requests.to_string()
    };
    let days = |n: u32| format!("{n} day{}", if n == 1 { "" } else { "s" });
    let pairs: Vec<(&str, String)> = vec![
        (
            "Favorite model",
            shorten(summary.favorite().unwrap_or("-"), 30),
        ),
        ("Total tokens", total),
        ("Requests", requests),
        ("Turns", summary.turns.to_string()),
        (
            "Active days",
            format!("{} of {}", summary.active_days, summary.span_days),
        ),
        ("Current streak", days(summary.current_streak)),
        ("Longest streak", days(summary.longest_streak)),
        (
            "Most active day",
            summary.most_active_day.map_or_else(
                || "-".to_owned(),
                |(day, _)| day.format("%b %-d").to_string(),
            ),
        ),
        (
            "Peak hour",
            summary
                .peak_hour
                .map_or_else(|| "-".to_owned(), |hour| format!("{hour:02}:00")),
        ),
        ("Longest turn", format_duration(summary.longest_turn_ms)),
    ];
    if width >= 90 {
        for pair in pairs.chunks(2) {
            lines.push(stat_line(pair, (width as usize).saturating_sub(4) / 2));
        }
    } else {
        for pair in &pairs {
            lines.push(stat_line(std::slice::from_ref(pair), width as usize));
        }
    }
    if let Some(fun) = fun_fact(summary.total_tokens()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("✦ {fun}"),
            Style::default().fg(crate::tui::theme::accent()),
        )));
    }
    lines
}

pub(in crate::tui) fn models_lines(
    view: &StatsView,
    width: u16,
    now_ts: i64,
) -> Vec<Line<'static>> {
    use crate::stats::compact_tokens;

    let summary = view.summary(now_ts);
    if summary.models.is_empty() {
        return vec![Line::from(Span::styled(
            "No models used yet in this range.",
            Style::default().fg(Color::Gray),
        ))];
    }
    let total = summary.total_tokens().max(1);
    let bar_width = if width >= 80 { 16 } else { 0 };
    let name_width = ((width as usize).saturating_sub(bar_width + 44)).clamp(12, 34);
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{:<name_width$} {:<bar$}{:>5}  {:>8}  {:>8}  {:>10}",
            "Model",
            "",
            "Share",
            "Tokens",
            "Requests",
            "Speed",
            bar = if bar_width > 0 { bar_width + 1 } else { 0 },
        ),
        Style::default().fg(Color::DarkGray),
    ))];
    for model in &summary.models {
        let share = model.tokens as f64 / total as f64;
        let filled = (share * bar_width as f64).round() as usize;
        let mut spans = vec![Span::styled(
            format!("{:<name_width$} ", shorten(&model.model, name_width)),
            Style::default().fg(Color::White),
        )];
        if bar_width > 0 {
            spans.push(Span::styled(
                "█".repeat(filled),
                Style::default().fg(crate::tui::theme::accent()),
            ));
            spans.push(Span::styled(
                "░".repeat(bar_width - filled),
                Style::default().fg(HEAT[0]),
            ));
            spans.push(Span::raw(" "));
        }
        let speed = model
            .tokens_per_second
            .map_or_else(|| "-".to_owned(), |rate| format!("{rate:.0} tok/s"));
        spans.push(Span::styled(
            format!(
                "{:>4.0}%  {:>8}  {:>8}  {:>10}",
                share * 100.0,
                compact_tokens(model.tokens),
                model.requests,
                speed
            ),
            Style::default().fg(Color::Gray),
        ));
        lines.push(Line::from(spans));
    }
    lines
}

pub(in crate::tui) fn draw_stats(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let Some(view) = app.stats_view.as_ref() else {
        return;
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Stats ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(crate::tui::theme::accent()))
        .style(Style::default().bg(crate::tui::theme::panel_alt()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 5 || inner.width < 20 {
        return;
    }
    let tab = |label: &'static str, selected: bool| {
        Span::styled(
            format!(" {label} "),
            if selected {
                Style::default()
                    .fg(crate::tui::theme::accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        )
    };
    let mut ranges = Vec::new();
    for (index, range) in RANGES.iter().enumerate() {
        if index > 0 {
            ranges.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        }
        ranges.push(Span::styled(
            range_label(*range),
            if *range == view.range {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));
    }
    let header = vec![
        Line::from(vec![
            tab("Overview", view.tab == StatsTab::Overview),
            tab("Models", view.tab == StatsTab::Models),
        ]),
        Line::from(ranges),
        Line::from(""),
    ];
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let mut body = Vec::new();
    if !app.settings.stats_enabled {
        body.push(Line::from(Span::styled(
            "Recording is off: nothing new is being saved. To turn it on, press r.",
            Style::default().fg(Color::Rgb(240, 210, 90)),
        )));
        body.push(Line::from(""));
    }
    body.extend(match view.tab {
        StatsTab::Overview => overview_lines(view, inner.width.saturating_sub(2), now_ts),
        StatsTab::Models => models_lines(view, inner.width.saturating_sub(2), now_ts),
    });
    // The tabs stay pinned; only the body below them scrolls.
    let content_width = inner.width.saturating_sub(2);
    let header_height = header.len() as u16;
    let body_height = inner.height.saturating_sub(1 + header_height);
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x + 1, inner.y, content_width, header_height),
    );
    let last_scroll = (body.len() as u16).saturating_sub(body_height);
    let scroll = view.scroll.get().min(last_scroll);
    view.scroll.set(scroll);
    frame.render_widget(
        Paragraph::new(body).scroll((scroll, 0)),
        Rect::new(
            inner.x + 1,
            inner.y + header_height,
            content_width,
            body_height,
        ),
    );
    let footer = if view.confirm_clear {
        Span::styled(
            "Delete all usage history? y/n",
            Style::default()
                .fg(Color::Rgb(235, 80, 80))
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "Tab switch tab   ←/→ range   ↑/↓ scroll   c clear history   Esc close",
            Style::default().fg(Color::DarkGray),
        )
    };
    frame.render_widget(
        Paragraph::new(footer),
        Rect::new(
            inner.x + 1,
            inner.bottom() - 1,
            inner.width.saturating_sub(2),
            1,
        ),
    );
}

#[cfg(test)]
mod tests {
    use crate::Settings;
    use crate::stats::{Outcome, Range, Record};
    use crate::tui::render::draw;
    use crate::tui::state::App;
    use crate::tui::stats_view::{StatsTab, StatsView, models_lines, overview_lines};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const NOW: i64 = 1_791_400_000;

    // Tests that touch the shared per-process stats file take this lock.
    static STATS_FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn record(days_ago: i64, model: &str, input: u64, output: u64, turn: &str) -> Record {
        Record {
            ts: NOW - days_ago * 86_400,
            turn: turn.to_owned(),
            provider: "MultiAI".to_owned(),
            model: model.to_owned(),
            input_tokens: input,
            output_tokens: output,
            estimated: false,
            duration_ms: 4_000,
            tool_calls: 1,
            outcome: Outcome::Done,
        }
    }

    fn view() -> StatsView {
        StatsView::open(vec![
            record(0, "deepseek/deepseek-v4-flash", 90_000, 10_000, "a"),
            record(0, "deepseek/deepseek-v4-flash", 40_000, 5_000, "a"),
            record(2, "qwen/qwen3.8-27b", 20_000, 2_000, "b"),
            record(40, "openai/gpt-oss-120b", 3_000, 500, "c"),
        ])
    }

    fn text(lines: &[ratatui::text::Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_with_view() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.settings.stats_enabled = true;
        app.stats_view = Some(view());
        app
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn a_short_terminal_scrolls_the_body_and_keeps_the_header() {
        let mut app = app_with_view();
        let top = screen(&app, 70, 12);
        assert!(top.contains("Overview"), "{top}");
        assert!(top.contains("Mon"), "starts at the heatmap: {top}");
        assert!(!top.contains("Longest turn"), "{top}");
        app.handle_stats_key(key(KeyCode::End)).expect("end");
        let bottom = screen(&app, 70, 12);
        assert!(bottom.contains("Overview"), "header stays pinned: {bottom}");
        assert!(
            bottom.contains("Longest turn"),
            "End reaches the last line: {bottom}"
        );
        assert!(!bottom.contains("Mon"), "{bottom}");
        app.handle_stats_key(key(KeyCode::Home)).expect("home");
        assert!(!screen(&app, 70, 12).contains("Longest turn"));
        app.handle_stats_key(key(KeyCode::Down)).expect("down");
        app.handle_stats_key(key(KeyCode::Up)).expect("up");
        let shown = screen(&app, 70, 12);
        assert!(shown.contains("Mon"), "{shown}");
    }

    #[test]
    fn changing_tab_or_range_returns_to_the_top() {
        let mut app = app_with_view();
        app.handle_stats_key(key(KeyCode::End)).expect("end");
        app.handle_stats_key(key(KeyCode::Right)).expect("range");
        assert!(screen(&app, 70, 12).contains("Mon"));
    }

    #[test]
    fn the_overview_lists_the_headline_numbers() {
        let shown = text(&overview_lines(&view(), 110, NOW));
        for expected in [
            "Favorite model",
            "deepseek/deepseek-v4-flash",
            "Total tokens",
            "170.5k",
            "Requests",
            "Turns",
            "Active days",
            "Current streak",
            "Longest streak",
            "Most active day",
            "Peak hour",
            "Longest turn",
        ] {
            assert!(
                shown.contains(expected),
                "missing {expected:?} in:\n{shown}"
            );
        }
    }

    #[test]
    fn the_overview_adds_a_fun_line_and_a_heatmap() {
        let shown = text(&overview_lines(&view(), 110, NOW));
        assert!(shown.contains("The Hobbit"), "{shown}");
        assert!(shown.contains('■'), "heatmap cells: {shown}");
        assert!(shown.contains("Mon"), "{shown}");
    }

    #[test]
    fn estimated_totals_are_marked_with_a_tilde() {
        let mut estimated = view();
        estimated.records[0].estimated = true;
        assert!(text(&overview_lines(&estimated, 110, NOW)).contains("~170.5k"));
    }

    #[test]
    fn ranges_change_what_is_counted() {
        let mut week = view();
        week.range = Range::Days(7);
        let shown = text(&overview_lines(&week, 110, NOW));
        assert!(shown.contains("167k") || shown.contains("167."), "{shown}");
        assert!(!shown.contains("170.5k"), "{shown}");
    }

    #[test]
    fn the_models_tab_ranks_models_with_shares_and_speed() {
        let shown = text(&models_lines(&view(), 110, NOW));
        let first = shown.find("deepseek/deepseek-v4-flash").expect("favorite");
        let second = shown.find("qwen/qwen3.8-27b").expect("second");
        let third = shown.find("openai/gpt-oss-120b").expect("third");
        assert!(first < second && second < third, "{shown}");
        assert!(shown.contains('%') && shown.contains("tok/s"), "{shown}");
    }

    #[test]
    fn keys_switch_tabs_and_ranges_and_close() {
        let mut app = app_with_view();
        app.handle_stats_key(key(KeyCode::Tab)).expect("tab");
        assert_eq!(app.stats_view.as_ref().unwrap().tab, StatsTab::Models);
        app.handle_stats_key(key(KeyCode::Tab)).expect("tab");
        assert_eq!(app.stats_view.as_ref().unwrap().tab, StatsTab::Overview);
        app.handle_stats_key(key(KeyCode::Right)).expect("right");
        assert_eq!(app.stats_view.as_ref().unwrap().range, Range::Days(30));
        app.handle_stats_key(key(KeyCode::Right)).expect("right");
        assert_eq!(app.stats_view.as_ref().unwrap().range, Range::Days(7));
        app.handle_stats_key(key(KeyCode::Right))
            .expect("right wraps");
        assert_eq!(app.stats_view.as_ref().unwrap().range, Range::All);
        app.handle_stats_key(key(KeyCode::Left))
            .expect("left wraps");
        assert_eq!(app.stats_view.as_ref().unwrap().range, Range::Days(7));
        app.handle_stats_key(key(KeyCode::Esc)).expect("esc");
        assert!(app.stats_view.is_none());
    }

    #[test]
    fn clearing_history_needs_confirmation() {
        let _guard = STATS_FILE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = app_with_view();
        app.handle_stats_key(key(KeyCode::Char('c'))).expect("c");
        assert!(app.stats_view.as_ref().unwrap().confirm_clear);
        app.handle_stats_key(key(KeyCode::Char('n'))).expect("n");
        assert!(!app.stats_view.as_ref().unwrap().confirm_clear);
        assert_eq!(app.stats_view.as_ref().unwrap().records.len(), 4);
        app.handle_stats_key(key(KeyCode::Char('c'))).expect("c");
        app.handle_stats_key(key(KeyCode::Char('y'))).expect("y");
        let view = app.stats_view.as_ref().expect("view stays open");
        assert!(view.records.is_empty() && !view.confirm_clear);
    }

    #[test]
    fn a_failed_delete_shows_a_message_and_keeps_the_session_alive() {
        let _guard = STATS_FILE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = crate::stats::stats_path().expect("path");
        // A directory where the file should be makes the deletion fail.
        std::fs::create_dir_all(&path).expect("blocker");
        let mut app = app_with_view();
        app.handle_stats_key(key(KeyCode::Char('c'))).expect("c");
        let result = app.handle_stats_key(key(KeyCode::Char('y')));
        std::fs::remove_dir_all(&path).expect("cleanup");
        assert!(result.is_ok(), "a failed delete must not end the session");
        assert!(app.notice.contains("Could not delete"), "{}", app.notice);
        let view = app.stats_view.as_ref().expect("view stays open");
        assert_eq!(
            view.records.len(),
            4,
            "history is kept when deleting failed"
        );
        assert!(!view.confirm_clear);
    }

    #[test]
    fn a_capital_y_also_confirms() {
        let _guard = STATS_FILE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = app_with_view();
        app.handle_stats_key(key(KeyCode::Char('c'))).expect("c");
        app.handle_stats_key(key(KeyCode::Char('Y'))).expect("Y");
        assert!(app.stats_view.as_ref().unwrap().records.is_empty());
    }

    #[test]
    fn switching_ranges_never_shows_a_stale_summary() {
        let mut view = view();
        let all = text(&overview_lines(&view, 110, NOW));
        view.range = Range::Days(7);
        let week = text(&overview_lines(&view, 110, NOW));
        view.range = Range::All;
        let again = text(&overview_lines(&view, 110, NOW));
        assert_ne!(all, week);
        assert_eq!(all, again);
        // New data invalidates the cache too.
        view.records.push(record(0, "new-model", 1_000_000, 0, "z"));
        assert!(text(&overview_lines(&view, 110, NOW)).contains("1.2M"));
    }

    #[test]
    fn the_screen_shows_tabs_ranges_and_the_empty_and_off_states() {
        let app = app_with_view();
        let shown = screen(&app, 120, 36);
        assert!(
            shown.contains("Overview") && shown.contains("Models"),
            "{shown}"
        );
        assert!(
            shown.contains("All time") && shown.contains("Last 7 days"),
            "{shown}"
        );
        assert!(shown.contains("Esc close"), "{shown}");

        let mut empty = App::new(Settings::default());
        empty.trust_prompt = false;
        empty.settings.stats_enabled = true;
        empty.stats_view = Some(StatsView::open(Vec::new()));
        assert!(screen(&empty, 120, 36).contains("No usage recorded yet"));

        empty.settings.stats_enabled = false;
        let off = screen(&empty, 120, 36);
        assert!(
            off.contains("Recording is off") && off.contains("press r"),
            "{off}"
        );
    }

    #[test]
    fn r_turns_recording_on() {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.stats_view = Some(StatsView::open(Vec::new()));
        assert!(!app.settings.stats_enabled);
        app.handle_stats_key(key(KeyCode::Char('r'))).expect("r");
        assert!(app.settings.stats_enabled && app.settings.stats_prompt_answered);
    }

    #[test]
    fn the_view_never_panics_on_tiny_terminals() {
        let app = app_with_view();
        for (width, height) in [(20, 5), (40, 10), (60, 14), (200, 60)] {
            let _ = screen(&app, width, height);
        }
        let mut models = app_with_view();
        models.stats_view.as_mut().unwrap().tab = StatsTab::Models;
        for (width, height) in [(20, 5), (40, 10), (60, 14)] {
            let _ = screen(&models, width, height);
        }
    }
}
