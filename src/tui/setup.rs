//! The first-run setup: a short sequence of questions shown over the welcome screen.
//!
//! Only questions that have not been answered yet are asked, so someone who set things up with an
//! earlier version sees just the new ones. Every answer is saved as it is given, and everything
//! can be changed later in Settings.

use crate::tui::state::App;
use crate::tui::theme::{self, THEMES};
use crate::{Settings, ThemeId, write_settings};
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum SetupStep {
    Theme,
    Motion,
    Stats,
    Sessions,
    Instructions,
}

pub(in crate::tui) struct SetupWizard {
    steps: Vec<SetupStep>,
    position: usize,
    choice: usize,
    /// The ticked boxes of the instruction-files step: project CLAUDE.md, project AGENTS.md,
    /// global CLAUDE.md.
    checks: [bool; 3],
    /// The theme in effect before the Theme step started previewing others.
    original_theme: ThemeId,
}

impl SetupWizard {
    /// The wizard for what is still unanswered, or `None` when everything has been answered.
    pub(in crate::tui) fn new(settings: &Settings) -> Option<SetupWizard> {
        let mut steps = Vec::new();
        if !settings.theme_prompt_answered {
            steps.push(SetupStep::Theme);
        }
        if !settings.motion_prompt_answered {
            steps.push(SetupStep::Motion);
        }
        if !settings.stats_prompt_answered {
            steps.push(SetupStep::Stats);
        }
        if !settings.sessions_prompt_answered {
            steps.push(SetupStep::Sessions);
        }
        if !settings.instructions_prompt_answered {
            steps.push(SetupStep::Instructions);
        }
        let first = *steps.first()?;
        Some(SetupWizard {
            choice: default_choice(first, settings),
            checks: [
                settings.default_load_claude_md,
                settings.default_load_agents_md,
                settings.load_global_claude_md,
            ],
            steps,
            position: 0,
            original_theme: settings.theme,
        })
    }

    fn step(&self) -> SetupStep {
        self.steps[self.position]
    }

    fn option_count(&self) -> usize {
        match self.step() {
            SetupStep::Theme => THEMES.len(),
            SetupStep::Instructions => 3,
            _ => 2,
        }
    }
}

/// The highlighted answer when a step opens: the safe one, or the current theme.
fn default_choice(step: SetupStep, settings: &Settings) -> usize {
    match step {
        SetupStep::Theme => THEMES
            .iter()
            .position(|entry| entry.id == settings.theme)
            .unwrap_or(0),
        _ => 0,
    }
}

impl App {
    pub(in crate::tui) fn start_setup(&mut self) {
        self.wizard = SetupWizard::new(&self.settings);
    }

    pub(in crate::tui) fn handle_setup_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(wizard) = self.wizard.as_mut() else {
            return Ok(());
        };
        let step = wizard.step();
        let last = wizard.option_count() - 1;
        match key.code {
            KeyCode::Up | KeyCode::Left => wizard.choice = wizard.choice.saturating_sub(1),
            KeyCode::Down | KeyCode::Right => wizard.choice = (wizard.choice + 1).min(last),
            KeyCode::Char(' ') if step == SetupStep::Instructions => {
                wizard.checks[wizard.choice] = !wizard.checks[wizard.choice];
            }
            KeyCode::Enter => return self.confirm_setup_step(),
            KeyCode::Esc => return self.skip_setup(),
            KeyCode::Char('y' | 'Y') if matches!(step, SetupStep::Stats | SetupStep::Sessions) => {
                wizard.choice = 1;
                return self.confirm_setup_step();
            }
            KeyCode::Char('n' | 'N') if matches!(step, SetupStep::Stats | SetupStep::Sessions) => {
                wizard.choice = 0;
                return self.confirm_setup_step();
            }
            KeyCode::Char('r' | 'R') if step == SetupStep::Motion => {
                wizard.choice = 1;
                return self.confirm_setup_step();
            }
            KeyCode::Char('k' | 'K') if step == SetupStep::Motion => {
                wizard.choice = 0;
                return self.confirm_setup_step();
            }
            _ => {}
        }
        // The theme is previewed as the highlight moves; it is saved on Enter.
        if step == SetupStep::Theme {
            self.settings.theme = THEMES[wizard.choice].id;
        }
        Ok(())
    }

    fn confirm_setup_step(&mut self) -> Result<()> {
        let Some(wizard) = self.wizard.as_ref() else {
            return Ok(());
        };
        let (step, choice, checks) = (wizard.step(), wizard.choice, wizard.checks);
        match step {
            SetupStep::Theme => {
                self.settings.theme = THEMES[choice].id;
                self.settings.theme_prompt_answered = true;
                write_settings(&self.settings)?;
            }
            SetupStep::Motion => self.answer_motion_prompt(choice == 1)?,
            SetupStep::Stats => self.answer_stats_prompt(choice == 1)?,
            SetupStep::Sessions => {
                self.settings.sessions_enabled = choice == 1;
                self.settings.sessions_prompt_answered = true;
                write_settings(&self.settings)?;
            }
            SetupStep::Instructions => {
                self.settings.default_load_claude_md = checks[0];
                self.settings.default_load_agents_md = checks[1];
                self.settings.load_global_claude_md = checks[2];
                self.settings.instructions_prompt_answered = true;
                write_settings(&self.settings)?;
            }
        }
        let Some(wizard) = self.wizard.as_mut() else {
            return Ok(());
        };
        wizard.position += 1;
        if wizard.position >= wizard.steps.len() {
            self.wizard = None;
            self.notice = "Setup finished. Everything can be changed in Settings.".to_owned();
        } else {
            wizard.choice = default_choice(wizard.steps[wizard.position], &self.settings);
        }
        Ok(())
    }

    /// Esc: take the safe answer for everything still open and move on.
    fn skip_setup(&mut self) -> Result<()> {
        let Some(wizard) = self.wizard.take() else {
            return Ok(());
        };
        self.settings.theme = wizard.original_theme;
        self.settings.theme_prompt_answered = true;
        self.settings.motion_prompt_answered = true;
        self.settings.stats_prompt_answered = true;
        self.settings.sessions_prompt_answered = true;
        self.settings.instructions_prompt_answered = true;
        write_settings(&self.settings)?;
        self.notice =
            "Setup skipped: animations on; stats, saved sessions and CLAUDE.md/AGENTS.md loading off. See Settings to change that."
                .to_owned();
        Ok(())
    }
}

/// What a step asks, explains and offers.
struct Prompt {
    heading: &'static str,
    detail: &'static str,
    options: Vec<String>,
}

fn prompt(step: SetupStep) -> Prompt {
    match step {
        SetupStep::Theme => Prompt {
            heading: "Pick a look",
            detail: "Move through the list to preview each theme. You can change it any time in Settings → Appearance.",
            options: THEMES.iter().map(|entry| entry.name.to_owned()).collect(),
        },
        SetupStep::Motion => Prompt {
            heading: "Do you prefer reduced motion?",
            detail: "Text pulses and the background animation can be turned off. You can change this later in Settings → General and Appearance.",
            options: vec!["Keep animations".to_owned(), "Reduce motion".to_owned()],
        },
        SetupStep::Stats => Prompt {
            heading: "Keep local usage stats?",
            detail: "Cool Code can record which models you use and how many tokens, to power /stats. It saves only counts, model names, and timestamps on this computer, in ~/.coolcode/stats.jsonl: never your prompts or answers. Nothing is sent anywhere.",
            options: vec!["No thanks".to_owned(), "Yes, record usage stats".to_owned()],
        },
        SetupStep::Sessions => Prompt {
            heading: "Save conversations so you can resume them?",
            detail: "Sessions are saved as files in ~/.coolcode/sessions, on this computer only, so `coolcode --resume` and /resume can continue them. They contain the full conversation, including tool output and anything you typed, exactly as typed (before privacy redaction).",
            options: vec!["No thanks".to_owned(), "Yes, save sessions".to_owned()],
        },
        SetupStep::Instructions => Prompt {
            heading: "Load instruction files?",
            detail: "Some projects keep guidance for AI assistants in CLAUDE.md or AGENTS.md. If ticked, that text is sent to the model with every request. Project files are only read in folders you trust; your own global file is always yours. Change it per project later with /claudemd and /agentsmd.",
            options: vec![
                "Load project CLAUDE.md".to_owned(),
                "Load project AGENTS.md".to_owned(),
                "Load my global CLAUDE.md (~/.claude/CLAUDE.md)".to_owned(),
            ],
        },
    }
}

pub(in crate::tui) fn draw_setup(frame: &mut ratatui::Frame<'_>, area: Rect, wizard: &SetupWizard) {
    let step = wizard.step();
    let prompt = prompt(step);
    let accent = theme::accent();
    let width = area.width.min(76);
    let detail_lines = (prompt.detail.chars().count() as u16 / width.saturating_sub(6).max(1)) + 1;
    let extra = if step == SetupStep::Theme { 2 } else { 0 };
    let wanted = 9 + detail_lines + prompt.options.len() as u16 + extra;
    let height = wanted.min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let title = format!(
        " Setup · {} of {} ",
        wizard.position + 1,
        wizard.steps.len()
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(theme::panel()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            prompt.heading,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            prompt.detail,
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
    ];
    for (index, label) in prompt.options.iter().enumerate() {
        let chosen = index == wizard.choice;
        if step == SetupStep::Instructions {
            let ticked = wizard.checks[index];
            lines.push(Line::from(vec![
                Span::styled(
                    if chosen { "▸ " } else { "  " },
                    Style::default().fg(accent),
                ),
                Span::styled(
                    if ticked { "[x] " } else { "[ ] " },
                    Style::default().fg(if ticked { accent } else { Color::DarkGray }),
                ),
                Span::styled(
                    label.clone(),
                    if chosen {
                        Style::default().fg(accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
            ]));
            continue;
        }
        let mut spans = vec![
            Span::styled(
                if chosen { "▸ " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(
                label.clone(),
                if chosen {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ];
        if step == SetupStep::Theme {
            spans.push(Span::styled(
                " ■",
                Style::default().fg(THEMES[index].accent),
            ));
        }
        lines.push(Line::from(spans));
    }
    if step == SetupStep::Theme {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            THEMES[wizard.choice].summary,
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if step == SetupStep::Instructions {
            "↑/↓ move   Space tick   Enter confirm   Esc skip setup"
        } else {
            "↑/↓ choose   Enter confirm   Esc skip setup"
        },
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::{SetupStep, SetupWizard};
    use crate::tui::render::draw;
    use crate::tui::state::App;
    use crate::{PulseMode, Settings, ThemeId};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn fresh() -> App {
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.start_setup();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_setup_key(key(code)).expect("key");
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

    fn steps(settings: &Settings) -> Vec<SetupStep> {
        SetupWizard::new(settings).map_or_else(Vec::new, |wizard| wizard.steps)
    }

    #[test]
    fn a_new_user_is_asked_everything_in_a_fixed_order() {
        assert_eq!(
            steps(&Settings::default()),
            [
                SetupStep::Theme,
                SetupStep::Motion,
                SetupStep::Stats,
                SetupStep::Sessions,
                SetupStep::Instructions
            ]
        );
    }

    #[test]
    fn someone_who_set_up_an_earlier_version_is_only_asked_the_new_questions() {
        let mut settings = Settings::default();
        settings.motion_prompt_answered = true;
        settings.stats_prompt_answered = true;
        assert_eq!(
            steps(&settings),
            [
                SetupStep::Theme,
                SetupStep::Sessions,
                SetupStep::Instructions
            ]
        );
        settings.theme_prompt_answered = true;
        settings.sessions_prompt_answered = true;
        settings.instructions_prompt_answered = true;
        assert!(SetupWizard::new(&settings).is_none(), "nothing left to ask");
    }

    #[test]
    fn pressing_enter_all_the_way_takes_the_safe_answers() {
        let mut app = fresh();
        for _ in 0..5 {
            press(&mut app, KeyCode::Enter);
        }
        assert!(app.wizard.is_none());
        let settings = &app.settings;
        assert_eq!(settings.theme, ThemeId::Cool);
        assert_eq!(settings.pulse, PulseMode::Words);
        assert!(settings.background_animation);
        assert!(!settings.stats_enabled, "stats stay opt-in");
        assert!(!settings.sessions_enabled, "sessions stay opt-in");
        assert!(
            !settings.default_load_claude_md
                && !settings.default_load_agents_md
                && !settings.load_global_claude_md,
            "instruction files stay opt-in"
        );
        assert!(
            settings.theme_prompt_answered
                && settings.motion_prompt_answered
                && settings.stats_prompt_answered
                && settings.sessions_prompt_answered
                && settings.instructions_prompt_answered
        );
        assert!(app.notice.contains("Setup finished"), "{}", app.notice);
    }

    #[test]
    fn saying_yes_to_stats_and_sessions_turns_them_on() {
        let mut app = fresh();
        press(&mut app, KeyCode::Enter); // theme
        press(&mut app, KeyCode::Enter); // motion
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // stats: yes
        assert!(app.settings.stats_enabled);
        press(&mut app, KeyCode::Char('y')); // sessions: yes
        assert!(app.settings.sessions_enabled && app.settings.sessions_prompt_answered);
        assert!(
            app.wizard.is_some(),
            "the instruction files are still to come"
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.wizard.is_none());
    }

    #[test]
    fn the_letter_shortcuts_answer_directly() {
        let mut app = fresh();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.settings.pulse, PulseMode::Off);
        assert!(!app.settings.background_animation && !app.settings.backdrop_in_chat);
        press(&mut app, KeyCode::Char('n'));
        assert!(app.settings.stats_prompt_answered && !app.settings.stats_enabled);
        press(&mut app, KeyCode::Char('n'));
        assert!(app.settings.sessions_prompt_answered && !app.settings.sessions_enabled);
    }

    #[test]
    fn the_theme_step_previews_as_you_move_and_saves_on_enter() {
        let mut app = fresh();
        press(&mut app, KeyCode::Down);
        assert_eq!(app.settings.theme, ThemeId::Galaxy, "previewed");
        assert!(!app.settings.theme_prompt_answered, "not saved yet");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.settings.theme, ThemeId::GalaxyVoid);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.theme, ThemeId::Galaxy);
        assert!(app.settings.theme_prompt_answered);
    }

    #[test]
    fn escape_skips_the_rest_and_undoes_a_theme_preview() {
        let mut app = fresh();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Esc);
        assert!(app.wizard.is_none());
        assert_eq!(app.settings.theme, ThemeId::Cool, "the preview is undone");
        let settings = &app.settings;
        assert!(
            settings.theme_prompt_answered
                && settings.motion_prompt_answered
                && settings.stats_prompt_answered
                && settings.sessions_prompt_answered,
            "skipped questions are not asked again"
        );
        assert!(!settings.stats_enabled && !settings.sessions_enabled);
        assert!(app.notice.contains("skipped"), "{}", app.notice);
    }

    #[test]
    fn answers_are_remembered_between_launches() {
        let mut app = fresh();
        press(&mut app, KeyCode::Enter);
        // Quit half-way: only the answered step is saved, the rest is asked next time.
        let reloaded = crate::read_settings().expect("settings file");
        assert!(reloaded.theme_prompt_answered);
        assert!(!reloaded.sessions_prompt_answered);
    }

    #[test]
    fn the_cursor_stays_inside_the_options() {
        let mut app = fresh();
        for _ in 0..30 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.settings.theme, ThemeId::Synthwave);
        for _ in 0..30 {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.settings.theme, ThemeId::Cool);
    }

    #[test]
    fn each_step_explains_what_it_does_and_counts_the_steps() {
        let mut app = fresh();
        let theme = screen(&app, 100, 34);
        assert!(theme.contains("Setup · 1 of 5"), "{theme}");
        assert!(
            theme.contains("Pick a look") && theme.contains("Galaxy (Void)"),
            "{theme}"
        );
        press(&mut app, KeyCode::Enter);
        assert!(screen(&app, 100, 34).contains("reduced motion"));
        press(&mut app, KeyCode::Enter);
        let stats = screen(&app, 100, 34);
        assert!(stats.contains("Setup · 3 of 5") && stats.contains("Keep local usage stats?"));
        assert!(stats.contains("never your prompts or answers"), "{stats}");
        press(&mut app, KeyCode::Enter);
        let sessions = screen(&app, 100, 34);
        assert!(sessions.contains("Save conversations"), "{sessions}");
        assert!(sessions.contains("before privacy redaction"), "{sessions}");
        assert!(sessions.contains("~/.coolcode/sessions"), "{sessions}");
        press(&mut app, KeyCode::Enter);
        let files = screen(&app, 100, 34);
        assert!(files.contains("Setup · 5 of 5"), "{files}");
        for label in [
            "Load project CLAUDE.md",
            "Load project AGENTS.md",
            "Load my global CLAUDE.md",
            "/claudemd and /agentsmd",
        ] {
            assert!(files.contains(label), "{label} missing:\n{files}");
        }
        assert!(files.contains("[ ]"), "unticked by default: {files}");
    }

    fn at_the_instructions_step() -> App {
        let mut app = fresh();
        for _ in 0..4 {
            press(&mut app, KeyCode::Enter);
        }
        app
    }

    #[test]
    fn space_ticks_the_boxes_and_enter_saves_them_as_the_defaults() {
        let mut app = at_the_instructions_step();
        press(&mut app, KeyCode::Char(' ')); // project CLAUDE.md
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' ')); // global CLAUDE.md
        let shown = screen(&app, 100, 34);
        assert_eq!(shown.matches("[x]").count(), 2, "{shown}");
        press(&mut app, KeyCode::Enter);
        let settings = &app.settings;
        assert!(settings.default_load_claude_md);
        assert!(!settings.default_load_agents_md);
        assert!(settings.load_global_claude_md);
        assert!(settings.instructions_prompt_answered);
        assert!(app.wizard.is_none());
    }

    #[test]
    fn a_box_can_be_unticked_again_and_space_does_nothing_elsewhere() {
        let mut app = at_the_instructions_step();
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);
        assert!(!app.settings.default_load_claude_md);
        let mut other = fresh();
        press(&mut other, KeyCode::Char(' '));
        assert!(
            other.wizard.is_some(),
            "space does not answer a normal step"
        );
    }

    #[test]
    fn the_ticks_start_from_what_is_already_chosen() {
        let mut settings = Settings::default();
        settings.default_load_agents_md = true;
        settings.theme_prompt_answered = true;
        settings.motion_prompt_answered = true;
        settings.stats_prompt_answered = true;
        settings.sessions_prompt_answered = true;
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app.start_setup();
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.default_load_agents_md, "kept as it was");
    }

    #[test]
    fn the_wizard_draws_on_tiny_terminals() {
        let app = fresh();
        for (w, h) in [(1, 1), (10, 4), (30, 8), (60, 14), (200, 60)] {
            let _ = screen(&app, w, h);
        }
    }
}
