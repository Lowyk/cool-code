//! Settings > General > Dynamic workflows: how many subagents a turn may start, and how many
//! may run at the same time.
//!
//! Sizes above 100 subagents ask once for confirmation that they can be very expensive, like the
//! Ultimate effort does; the answer is remembered in the settings.

use crate::tui::effort::effort_name;
use crate::tui::render::centered_rect;
use crate::tui::settings::SettingsView;
use crate::tui::state::App;
use crate::workflow::{CUSTOM_CEILING, MAX_AT_ONCE, WorkflowSize};
use crate::write_settings;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

/// The row below the sizes.
const AT_ONCE_ROW: usize = WorkflowSize::ALL.len();
/// The row of the Custom size, the last one.
const CUSTOM_ROW: usize = AT_ONCE_ROW - 1;
/// The Custom number has at most this many digits (the ceiling is 500).
const CUSTOM_DIGITS: usize = 3;

/// The chooser while it is open (it replaces the General rows).
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct WorkflowChooser {
    /// The highlighted row: one per size, then the At once row.
    pub(in crate::tui) row: usize,
    /// The number typed for the Custom size.
    pub(in crate::tui) custom: String,
    /// A size waiting for the "very expensive" confirmation, with its Custom number.
    pub(in crate::tui) confirm: Option<(WorkflowSize, usize)>,
}

impl WorkflowChooser {
    /// Opens on the current size, with the current Custom number ready to edit.
    pub(in crate::tui) fn open(settings: &crate::Settings) -> WorkflowChooser {
        WorkflowChooser {
            row: WorkflowSize::ALL
                .iter()
                .position(|size| *size == settings.workflow_size)
                .unwrap_or(0),
            custom: settings
                .workflow_custom_size
                .clamp(1, CUSTOM_CEILING)
                .to_string(),
            confirm: None,
        }
    }
}

pub(super) fn chooser_hint(chooser: &WorkflowChooser) -> &'static str {
    if chooser.confirm.is_some() {
        "y confirm   n/Esc cancel"
    } else if chooser.row == AT_ONCE_ROW {
        "↑↓ move   ←/→ change   Esc back"
    } else if chooser.row == CUSTOM_ROW {
        "↑↓ move   type a number   Enter choose   Esc back"
    } else {
        "↑↓ move   Enter choose   Esc back"
    }
}

/// What a size row says after its name.
fn size_detail(size: WorkflowSize, chooser: &WorkflowChooser) -> String {
    match size.preset_limit() {
        Some(0) => "no workflows; Super and Ultimate stay locked".to_owned(),
        Some(limit) => format!("up to {limit}"),
        None => format!(
            "up to [{}{}]  (1 to {CUSTOM_CEILING})",
            chooser.custom,
            if chooser.row == CUSTOM_ROW { "_" } else { "" }
        ),
    }
}

pub(super) fn draw_workflow_chooser(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let Some(chooser) = view.workflow_chooser.as_ref() else {
        return;
    };
    let accent = crate::tui::theme::accent();
    let row = |index: usize, name: &str, detail: String, current: bool| {
        let selected = index == chooser.row;
        Line::from(vec![
            Span::styled(
                if selected { "▸ " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(
                format!("{name:<10}"),
                if selected {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::styled(detail, Style::default().fg(Color::DarkGray)),
            Span::styled(
                if current { "  (current)" } else { "" },
                Style::default().fg(Color::Rgb(110, 220, 130)),
            ),
        ])
    };
    let mut lines = vec![
        Line::from(Span::styled(
            "Dynamic workflows: the most subagents one turn may start",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (index, size) in WorkflowSize::ALL.iter().copied().enumerate() {
        lines.push(row(
            index,
            size.label(),
            size_detail(size, chooser),
            size == app.settings.workflow_size,
        ));
    }
    lines.push(row(
        AT_ONCE_ROW,
        "At once",
        format!(
            "‹ {} ›  running at the same time (1 to {MAX_AT_ONCE})",
            app.settings.subagents_at_once()
        ),
        false,
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Ultimate may use the whole size, Super half, and Low, Medium or High with workflows a quarter. Every subagent makes its own requests to your provider.",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    if let Some((size, custom)) = chooser.confirm {
        draw_confirmation(frame, area, size, custom);
    }
}

/// The one-time warning before a size above 100 subagents.
fn draw_confirmation(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    size: WorkflowSize,
    custom: usize,
) {
    let limit = size.preset_limit().unwrap_or(custom);
    let popup = centered_rect(80, 60, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(format!(" Confirm {} workflows ", size.label()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .style(Style::default().bg(crate::tui::theme::dialog()));
    let body = Paragraph::new(vec![
        Line::from(format!(
            "This lets one turn start up to {limit} subagents. That can be very expensive: each one makes its own requests, and one turn can use many times the tokens of a normal one."
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Y",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm    "),
            Span::styled("N / Esc", Style::default().fg(Color::White)),
            Span::raw(" cancel"),
        ]),
    ])
    .wrap(Wrap { trim: true })
    .block(block);
    frame.render_widget(body, popup);
}

impl App {
    pub(super) fn handle_workflow_chooser_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(mut chooser) = self
            .settings_view
            .as_ref()
            .and_then(|view| view.workflow_chooser.clone())
        else {
            return Ok(());
        };
        if let Some((size, custom)) = chooser.confirm {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    self.settings.large_workflows_acknowledged = true;
                    return self.choose_workflow_size(size, custom);
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    chooser.confirm = None;
                    self.notice = "Workflow size unchanged.".to_owned();
                }
                _ => {}
            }
            self.keep_chooser(Some(chooser));
            return Ok(());
        }
        match key.code {
            KeyCode::Up => chooser.row = chooser.row.saturating_sub(1),
            KeyCode::Down => chooser.row = (chooser.row + 1).min(AT_ONCE_ROW),
            KeyCode::Esc => {
                self.keep_chooser(None);
                return Ok(());
            }
            KeyCode::Left | KeyCode::Right if chooser.row == AT_ONCE_ROW => {
                let now = self.settings.subagents_at_once();
                self.settings.workflow_at_once = if key.code == KeyCode::Left {
                    now.saturating_sub(1).max(1)
                } else {
                    (now + 1).min(MAX_AT_ONCE)
                };
                write_settings(&self.settings)?;
            }
            KeyCode::Char(digit)
                if chooser.row == CUSTOM_ROW
                    && digit.is_ascii_digit()
                    && chooser.custom.len() < CUSTOM_DIGITS =>
            {
                chooser.custom.push(digit);
            }
            KeyCode::Backspace if chooser.row == CUSTOM_ROW => {
                chooser.custom.pop();
            }
            KeyCode::Enter if chooser.row < AT_ONCE_ROW => {
                let size = WorkflowSize::ALL[chooser.row];
                let custom = if size == WorkflowSize::Custom {
                    match chooser.custom.parse::<usize>() {
                        Ok(number) if (1..=CUSTOM_CEILING).contains(&number) => number,
                        _ => {
                            self.notice = format!(
                                "Type a number from 1 to {CUSTOM_CEILING} for the Custom size."
                            );
                            self.keep_chooser(Some(chooser));
                            return Ok(());
                        }
                    }
                } else {
                    self.settings.workflow_custom_size
                };
                if size.needs_confirmation(custom) && !self.settings.large_workflows_acknowledged {
                    chooser.confirm = Some((size, custom));
                    self.keep_chooser(Some(chooser));
                    return Ok(());
                }
                return self.choose_workflow_size(size, custom);
            }
            _ => {}
        }
        self.keep_chooser(Some(chooser));
        Ok(())
    }

    fn keep_chooser(&mut self, chooser: Option<WorkflowChooser>) {
        if let Some(view) = self.settings_view.as_mut() {
            view.workflow_chooser = chooser;
        }
    }

    /// Saves the chosen size, closes the chooser and says what it means. Off puts a workflow
    /// tier back to its model level.
    fn choose_workflow_size(&mut self, size: WorkflowSize, custom: usize) -> Result<()> {
        self.settings.workflow_size = size;
        if size == WorkflowSize::Custom {
            self.settings.workflow_custom_size = custom;
        }
        self.notice = if size != WorkflowSize::Off {
            format!(
                "Dynamic workflows unlocked: {}, up to {} subagents a turn. Super, Ultimate and workflows on lower levels can use many more tokens.",
                size.label(),
                self.settings.workflow_limit()
            )
        } else if self.settings.enforce_workflow_lock() {
            format!(
                "Dynamic workflows off and locked; effort set to {}.",
                effort_name(self.settings.effort)
            )
        } else {
            "Dynamic workflows off and locked.".to_owned()
        };
        write_settings(&self.settings)?;
        self.keep_chooser(None);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::tui::render::draw;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crate::workflow::WorkflowSize;
    use crate::{Effort, Settings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn typed(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app, 0)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// An app with the chooser open, its cursor on the first row.
    fn chooser(settings: Settings) -> App {
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.open_settings(Section::General);
        press(&mut app, KeyCode::Right);
        for _ in 0..11 {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Enter);
        assert!(
            app.settings_view
                .as_ref()
                .is_some_and(|view| view.workflow_chooser.is_some()),
            "Enter on Dynamic workflows opens the chooser"
        );
        while app
            .settings_view
            .as_ref()
            .and_then(|view| view.workflow_chooser.as_ref())
            .is_some_and(|chooser| chooser.row > 0)
        {
            press(&mut app, KeyCode::Up);
        }
        app
    }

    fn open(app: &App) -> bool {
        app.settings_view
            .as_ref()
            .is_some_and(|view| view.workflow_chooser.is_some())
    }

    fn asking(app: &App) -> bool {
        app.settings_view
            .as_ref()
            .and_then(|view| view.workflow_chooser.as_ref())
            .is_some_and(|chooser| chooser.confirm.is_some())
    }

    /// Moves to the row of `size` (the rows are in [`WorkflowSize::ALL`] order).
    fn go_to(app: &mut App, size: WorkflowSize) {
        let index = WorkflowSize::ALL.iter().position(|s| *s == size).unwrap();
        for _ in 0..index {
            press(app, KeyCode::Down);
        }
    }

    #[test]
    fn the_general_row_shows_the_size_and_the_chooser_lists_every_size() {
        let mut settings = Settings::default();
        let app = {
            let mut app = App::new(settings.clone());
            app.trust_prompt = false;
            app.open_settings(Section::General);
            app
        };
        let general = screen(&app, 100, 30);
        assert!(general.contains("Dynamic workflows"), "{general}");
        settings.workflow_size = WorkflowSize::Big;
        let mut app = App::new(settings.clone());
        app.trust_prompt = false;
        app.open_settings(Section::General);
        let general = screen(&app, 100, 30);
        assert!(general.contains("Big · up to 30 · 8 at once"), "{general}");

        let app = chooser(settings);
        let shown = screen(&app, 100, 30);
        for expected in [
            "Off",
            "Small",
            "up to 5",
            "Medium",
            "up to 15",
            "Big",
            "Large",
            "up to 50",
            "Massive",
            "up to 100",
            "Extreme",
            "up to 200",
            "Custom",
            "1 to 500",
            "At once",
        ] {
            assert!(shown.contains(expected), "{expected} missing:\n{shown}");
        }
    }

    #[test]
    fn choosing_a_size_unlocks_the_tiers_and_off_locks_them_again() {
        let mut app = chooser(Settings::default());
        go_to(&mut app, WorkflowSize::Medium);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.workflow_size, WorkflowSize::Medium);
        assert!(app.settings.workflows_unlocked());
        assert!(!open(&app), "choosing closes the chooser");
        assert!(app.notice.contains("up to 15"), "{}", app.notice);
        let saved = crate::read_settings().expect("saved");
        assert_eq!(saved.workflow_size, WorkflowSize::Medium);

        let mut settings = Settings::default();
        settings.workflow_size = WorkflowSize::Medium;
        settings.effort = Effort::Ultimate;
        let mut app = chooser(settings);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.workflow_size, WorkflowSize::Off);
        assert_eq!(app.settings.effort, Effort::Max, "dropped out of Ultimate");
        assert!(app.notice.contains("locked"), "{}", app.notice);
    }

    #[test]
    fn massive_asks_once_and_the_answer_is_remembered() {
        let mut app = chooser(Settings::default());
        go_to(&mut app, WorkflowSize::Massive);
        press(&mut app, KeyCode::Enter);
        assert!(asking(&app), "Massive asks first");
        assert_eq!(app.settings.workflow_size, WorkflowSize::Off, "not yet");
        let shown = screen(&app, 100, 30);
        assert!(shown.contains("very expensive"), "{shown}");
        press(&mut app, KeyCode::Char('n'));
        assert!(!asking(&app) && open(&app), "back in the chooser");
        assert_eq!(app.settings.workflow_size, WorkflowSize::Off);
        assert!(!app.settings.large_workflows_acknowledged);

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert!(!asking(&app));
        assert_eq!(app.settings.workflow_size, WorkflowSize::Off, "Esc cancels");

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.settings.workflow_size, WorkflowSize::Massive);
        assert!(app.settings.large_workflows_acknowledged);
        assert!(crate::read_settings().unwrap().large_workflows_acknowledged);

        let mut app = chooser(app.settings.clone());
        go_to(&mut app, WorkflowSize::Extreme);
        press(&mut app, KeyCode::Enter);
        assert!(!asking(&app), "asked only once");
        assert_eq!(app.settings.workflow_size, WorkflowSize::Extreme);
    }

    #[test]
    fn small_sizes_never_ask() {
        for size in [
            WorkflowSize::Small,
            WorkflowSize::Medium,
            WorkflowSize::Big,
            WorkflowSize::Large,
        ] {
            let mut app = chooser(Settings::default());
            go_to(&mut app, size);
            press(&mut app, KeyCode::Enter);
            assert!(!asking(&app), "{size:?}");
            assert_eq!(app.settings.workflow_size, size);
        }
    }

    #[test]
    fn a_custom_size_is_typed_checked_and_confirmed_above_a_hundred() {
        let mut app = chooser(Settings::default());
        go_to(&mut app, WorkflowSize::Custom);
        for _ in 0..4 {
            press(&mut app, KeyCode::Backspace);
        }
        typed(&mut app, "80");
        press(&mut app, KeyCode::Enter);
        assert!(!asking(&app), "80 is not above 100");
        assert_eq!(app.settings.workflow_size, WorkflowSize::Custom);
        assert_eq!(app.settings.workflow_limit(), 80);

        for bad in ["0", "501", ""] {
            let mut app = chooser(Settings::default());
            go_to(&mut app, WorkflowSize::Custom);
            for _ in 0..4 {
                press(&mut app, KeyCode::Backspace);
            }
            typed(&mut app, bad);
            press(&mut app, KeyCode::Enter);
            assert_eq!(app.settings.workflow_size, WorkflowSize::Off, "{bad:?}");
            assert!(open(&app) && !asking(&app));
            assert!(app.notice.contains("1 to 500"), "{bad:?}: {}", app.notice);
        }

        let mut app = chooser(Settings::default());
        go_to(&mut app, WorkflowSize::Custom);
        for _ in 0..4 {
            press(&mut app, KeyCode::Backspace);
        }
        typed(&mut app, "12x5");
        let typed_text = app
            .settings_view
            .as_ref()
            .and_then(|view| view.workflow_chooser.as_ref())
            .map(|chooser| chooser.custom.clone());
        assert_eq!(typed_text.as_deref(), Some("125"), "only digits are typed");
        press(&mut app, KeyCode::Enter);
        assert!(asking(&app), "above 100 asks first");
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.settings.workflow_limit(), 125);
        assert!(app.settings.large_workflows_acknowledged);
    }

    #[test]
    fn at_once_is_changed_with_the_arrow_keys_between_one_and_thirty_two() {
        let mut app = chooser(Settings::default());
        for _ in 0..WorkflowSize::ALL.len() {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Right);
        assert_eq!(app.settings.workflow_at_once, 9);
        assert_eq!(crate::read_settings().unwrap().workflow_at_once, 9);
        for _ in 0..40 {
            press(&mut app, KeyCode::Right);
        }
        assert_eq!(app.settings.workflow_at_once, 32);
        for _ in 0..40 {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.settings.workflow_at_once, 1);
        assert!(open(&app), "the arrows do not leave the chooser");
        press(&mut app, KeyCode::Esc);
        assert!(!open(&app) && app.settings_view.is_some(), "Esc goes back");
    }

    #[test]
    fn the_chooser_draws_on_tiny_terminals() {
        let mut app = chooser(Settings::default());
        for (width, height) in [(20, 5), (12, 4), (40, 8), (69, 12)] {
            let _ = screen(&app, width, height);
        }
        go_to(&mut app, WorkflowSize::Massive);
        press(&mut app, KeyCode::Enter);
        for (width, height) in [(20, 5), (12, 4), (40, 8), (69, 12)] {
            let _ = screen(&app, width, height);
        }
    }
}
