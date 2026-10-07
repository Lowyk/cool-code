//! `/usage`: every provider's remaining usage in one place.
//!
//! Providers with a usage endpoint show live numbers (loaded in the background and reused for a
//! minute). The others cannot report usage to a normal API key, so the view points to the
//! provider's own dashboard instead of guessing.

use crate::ProviderProfile;
use crate::tui::render::centered_rect;
use crate::tui::settings::sync::LimitsState;
use crate::tui::state::App;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(in crate::tui) struct UsageView {
    scroll: u16,
}

/// Where a provider shows usage, for providers whose API keys cannot read it.
fn dashboard(profile: &ProviderProfile) -> Option<&'static str> {
    match profile.adapter.as_str() {
        "openai" => Some("platform.openai.com/usage"),
        "anthropic" => Some("console.anthropic.com/settings/usage"),
        "google" => Some("aistudio.google.com"),
        "groq" => Some("console.groq.com/dashboard"),
        _ => None,
    }
}

impl App {
    /// Opens the view and starts loading usage for every provider that can report it.
    pub(in crate::tui) fn open_usage(&mut self) {
        self.usage_view = Some(UsageView { scroll: 0 });
        self.load_all_usage();
    }

    fn load_all_usage(&mut self) {
        for index in 0..self.settings.providers.len() {
            if !self.settings.providers[index].draft {
                self.start_limits_fetch(index, false);
            }
        }
    }

    pub(in crate::tui) fn handle_usage_key(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.usage_view = None,
            KeyCode::Char('r') => {
                // Forgetting what was loaded makes the next load go to the provider again.
                self.limits.clear();
                self.load_all_usage();
            }
            KeyCode::Up => {
                if let Some(view) = self.usage_view.as_mut() {
                    view.scroll = view.scroll.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(view) = self.usage_view.as_mut() {
                    view.scroll = view.scroll.saturating_add(1);
                }
            }
            _ => {}
        }
    }
}

fn usage_lines(app: &App) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let label = Style::default().fg(Color::Gray);
    let mut lines = Vec::new();
    if app.settings.providers.is_empty() {
        lines.push(Line::from(Span::styled(
            "No providers yet. Add one with /provider.",
            dim,
        )));
    }
    for profile in &app.settings.providers {
        let is_default = app.settings.default_provider_id.as_deref() == Some(profile.id.as_str());
        let mut title = vec![Span::styled(
            profile.name.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )];
        if is_default {
            title.push(Span::styled("  default", dim));
        }
        lines.push(Line::from(title));
        let indent =
            |text: String, style: Style| Line::from(Span::styled(format!("  {text}"), style));
        if profile.draft {
            lines.push(indent("draft · finish its setup first".to_owned(), dim));
        } else if profile.limits_url.is_none() {
            let hint = dashboard(profile)
                .map(|host| format!(" ({host})"))
                .unwrap_or_default();
            lines.push(indent(
                format!(
                    "no usage API for this provider · check your provider's API dashboard{hint}"
                ),
                dim,
            ));
        } else {
            match app.limits.get(&profile.id).map(|entry| &entry.state) {
                None | Some(LimitsState::Loading) => lines.push(indent("loading…".to_owned(), dim)),
                Some(LimitsState::Failed(error)) => {
                    let shown: String = error.chars().take(100).collect();
                    lines.push(indent(format!("unavailable: {shown}"), dim));
                }
                Some(LimitsState::Ready(rows)) => {
                    for row in rows {
                        let mut spans = vec![
                            Span::styled(format!("  {:<9}", row.label), label),
                            Span::raw(row.value.clone()),
                        ];
                        if let Some(fraction) = row.remaining {
                            spans.push(Span::raw("  "));
                            spans.push(crate::tui::settings::remaining_bar(fraction));
                        }
                        lines.push(Line::from(spans));
                    }
                }
            }
        }
        lines.push(Line::from(""));
    }
    lines
}

pub(in crate::tui) fn draw_usage(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let Some(view) = app.usage_view.as_ref() else {
        return;
    };
    let popup = centered_rect(82, 80, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Usage ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(crate::tui::theme::accent_bright()))
        .style(Style::default().bg(crate::tui::theme::panel()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 3 {
        return;
    }
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    frame.render_widget(
        Paragraph::new(usage_lines(app))
            .wrap(Wrap { trim: false })
            .scroll((view.scroll, 0)),
        body,
    );
    frame.render_widget(
        Paragraph::new("↑/↓ scroll · r refresh · Esc close")
            .style(Style::default().fg(Color::DarkGray)),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoints::LimitLine;
    use crate::tui::settings::sync::{LimitsEntry, LimitsState};
    use crate::{ProviderProfile, Settings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn profile(id: &str, adapter: &str, limits: bool) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: format!("{id} provider"),
            adapter: adapter.to_owned(),
            base_url: Some("https://api.example.com/v1".to_owned()),
            limits_url: limits.then(|| "https://api.example.com/v1/limits".to_owned()),
            model: "m".to_owned(),
            ..Default::default()
        }
    }

    fn app_with(providers: Vec<ProviderProfile>) -> App {
        let mut settings = Settings::default();
        settings.providers = providers;
        let mut app = App::new(settings);
        app.trust_prompt = false;
        app
    }

    fn screen(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 40)).expect("terminal");
        terminal
            .draw(|frame| crate::tui::render::draw(frame, app, 0))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn the_command_opens_the_view_and_loads_every_provider_that_can_report() {
        let mut app = app_with(vec![
            profile("a", "openai-compatible", true),
            profile("b", "openai", false),
            profile("c", "openai-compatible", true),
        ]);
        app.input = "/usage".to_owned();
        app.submit().expect("usage");
        assert!(app.usage_view.is_some());
        assert_eq!(app.spawned_tasks, 2, "only providers with a usage endpoint");
    }

    #[test]
    fn providers_without_a_usage_api_point_to_their_dashboard() {
        let mut app = app_with(vec![
            profile("openai-key", "openai", false),
            profile("claude", "anthropic", false),
            profile("custom", "openai-compatible", false),
        ]);
        app.open_usage();
        let shown = screen(&app);
        assert!(
            shown.contains("check your provider's API dashboard"),
            "{shown}"
        );
        assert!(shown.contains("platform.openai.com/usage"), "{shown}");
        assert!(
            shown.contains("console.anthropic.com/settings/usage"),
            "{shown}"
        );
    }

    #[test]
    fn loaded_usage_shows_each_window_with_a_bar_and_failures_say_why() {
        let mut app = app_with(vec![
            profile("a", "openai-compatible", true),
            profile("b", "openai-compatible", true),
            profile("c", "openai-compatible", true),
        ]);
        app.open_usage();
        let entry = |state| LimitsEntry {
            fetched_at: std::time::Instant::now(),
            state,
        };
        app.limits.insert(
            "a".to_owned(),
            entry(LimitsState::Ready(vec![LimitLine {
                label: "5-hour".to_owned(),
                value: "27% used".to_owned(),
                remaining: Some(0.73),
                balance_tokens: None,
            }])),
        );
        app.limits.insert(
            "b".to_owned(),
            entry(LimitsState::Failed("provider returned 401".to_owned())),
        );
        let shown = screen(&app);
        assert!(shown.contains("27% used") && shown.contains('█'), "{shown}");
        assert!(
            shown.contains("unavailable: provider returned 401"),
            "{shown}"
        );
        assert!(shown.contains("loading…"), "c is still loading: {shown}");
    }

    #[test]
    fn drafts_are_listed_but_not_contacted_and_r_refreshes() {
        let mut draft = profile("d", "openai-compatible", true);
        draft.draft = true;
        let mut app = app_with(vec![draft, profile("a", "openai-compatible", true)]);
        app.open_usage();
        assert_eq!(app.spawned_tasks, 1);
        assert!(screen(&app).contains("draft"));
        app.limits.get_mut("a").unwrap().state = LimitsState::Ready(Vec::new());
        app.handle_usage_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(app.spawned_tasks, 2, "refreshing goes back to the provider");
    }

    #[test]
    fn escape_closes_the_view() {
        let mut app = app_with(Vec::new());
        app.open_usage();
        assert!(screen(&app).contains("No providers yet"));
        app.handle_usage_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.usage_view.is_none());
    }
}
