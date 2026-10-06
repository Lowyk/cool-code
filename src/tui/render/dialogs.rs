use crate::agent::ToolApproval;
use crate::policy::{MODES, mode_label};
use crate::tui::render::centered_rect;
use crate::tui::state::{App, PrivacyPrompt};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(super) fn draw_tool_approval(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    approval: &ToolApproval,
    scroll: u16,
) {
    let popup = centered_rect(88, 82, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(format!(" Approve action · {} ", approval.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(255, 197, 92)))
        .style(Style::default().bg(Color::Rgb(35, 31, 26)))
        .padding(ratatui::widgets::Padding::horizontal(2));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let lines = vec![
        Line::from(Span::styled(
            "This action is waiting for your approval.",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(approval.details.as_str()),
        Line::from(""),
        Line::from(Span::styled(
            "Y/Enter approve · N/Esc decline · ↑/↓ review details",
            Style::default().fg(Color::Rgb(255, 197, 92)),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}

pub(super) fn draw_privacy_confirmation(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    prompt: &PrivacyPrompt,
    has_image: bool,
) {
    let popup = centered_rect(82, 68, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Privacy check ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(255, 197, 92)))
        .style(Style::default().bg(Color::Rgb(35, 31, 26)))
        .padding(ratatui::widgets::Padding::horizontal(2));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let lines = vec![
        Line::from(Span::styled(
            format!(
                "{} requests send prompt context to that provider.",
                prompt.risk
            ),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(
            "Before sending, Cool Code locally redacts detected API keys, email addresses, phone-like numbers, and your custom values. The reversible mapping stays in memory and is never sent; matching placeholders in the reply are restored locally.",
        ),
        Line::from(""),
        Line::from(
            "This is best-effort, not a guarantee. Image contents are not scanned or redacted and may expose sensitive information.",
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                if prompt.allow_images {
                    "[✓] "
                } else {
                    "[ ] "
                },
                Style::default().fg(if prompt.allow_images {
                    Color::Green
                } else {
                    Color::Gray
                }),
            ),
            Span::styled(
                "Allow image contents to be sent unredacted",
                Style::default().fg(Color::White),
            ),
        ]),
        if has_image && !prompt.allow_images {
            Line::from(Span::styled(
                "An image is attached; enable this option before continuing.",
                Style::default().fg(Color::Rgb(255, 197, 92)),
            ))
        } else {
            Line::from("")
        },
        Line::from(""),
        Line::from(Span::styled(
            "←/→ or Space toggle images · Y/Enter acknowledge and send · N/Esc cancel",
            Style::default().fg(Color::Rgb(255, 197, 92)),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

pub(super) fn draw_workspace_trust_prompt(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let popup = centered_rect(78, 62, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Trust this workspace? ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(255, 197, 92)))
        .style(Style::default().bg(Color::Rgb(35, 31, 26)))
        .padding(ratatui::widgets::Padding::horizontal(2));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let root = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "(unknown folder)".to_owned());
    let cool_file = std::env::current_dir()
        .ok()
        .is_some_and(|path| path.join("COOL.md").is_file());
    let cool_dir = std::env::current_dir()
        .ok()
        .is_some_and(|path| path.join(".coolcode").is_dir());
    let lines = vec![
        Line::from(Span::styled(
            "Cool Code has not recorded trust for this folder yet.",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(root),
        Line::from(format!(
            "COOL.md: {}   ·   .coolcode/: {}",
            if cool_file { "present" } else { "not present" },
            if cool_dir { "present" } else { "not present" }
        )),
        Line::from(""),
        Line::from(
            "Trusting enables repository reads and permission-controlled exact-snippet edits and shell commands in this folder. The active permission mode determines which actions auto-run and which require your explicit approval. COOL.md and explicit @path files can be read only in a trusted workspace.",
        ),
        Line::from(
            "Declining keeps chat available but disables COOL.md and @path file reads. You can change this later in Settings → Privacy.",
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                if app.trust_choice == 0 {
                    "[ Yes, trust this folder ]"
                } else {
                    "  Yes, trust this folder  "
                },
                Style::default().fg(if app.trust_choice == 0 {
                    Color::Green
                } else {
                    Color::Gray
                }),
            ),
            Span::raw("    "),
            Span::styled(
                if app.trust_choice == 1 {
                    "[ No ]"
                } else {
                    " No "
                },
                Style::default().fg(if app.trust_choice == 1 {
                    Color::Rgb(255, 197, 92)
                } else {
                    Color::Gray
                }),
            ),
        ]),
        Line::from(Span::styled(
            "←/→ choose · Enter confirm · Y/N quick keys",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

pub(super) fn draw_model_provider_picker(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let choices = app
        .model_choices
        .as_ref()
        .expect("model provider picker open");
    let popup = centered_rect(62, (choices.len() as u16 * 3 + 8).min(70), area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Choose provider ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(98, 213, 244)))
        .style(Style::default().bg(Color::Rgb(29, 30, 32)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let model = app.pending_model.as_deref().unwrap_or("model");
    let mut lines = vec![
        Line::from(format!("`{model}` is available from multiple providers:")),
        Line::from(""),
    ];
    for (index, (provider_index, name, _model_id)) in choices.iter().enumerate() {
        let profile = &app.settings.providers[*provider_index];
        lines.push(Line::from(vec![
            Span::styled(
                if index == app.model_choice_index {
                    "› "
                } else {
                    "  "
                },
                Style::default().fg(Color::Rgb(98, 213, 244)),
            ),
            Span::styled(name, Style::default().fg(Color::White)),
            Span::styled(
                format!("  ·  {}", profile.adapter),
                Style::default().fg(Color::Gray),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ choose · Enter select · Esc cancel",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

pub(super) fn draw_mode_picker(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let popup = centered_rect(72, 34, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Permission mode ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(120, 220, 245)))
        .style(Style::default().bg(Color::Rgb(25, 32, 38)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let options = MODES.iter().enumerate().map(|(index, (label, _))| {
        if index == app.mode_index {
            Span::styled(
                format!("[ {label} ]"),
                Style::default()
                    .fg(Color::Rgb(120, 220, 245))
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(*label, Style::default().fg(Color::Gray))
        }
    });
    let mut spans = Vec::new();
    for (index, option) in options.enumerate() {
        if index > 0 {
            spans.push(Span::raw("   ·   "));
        }
        spans.push(option);
    }
    let selected = MODES[app.mode_index].1;
    let lines = vec![
        Line::from(""),
        Line::from(spans),
        Line::from(""),
        Line::from(Span::styled(
            format!("Current: {}", mode_label(selected)),
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "←/→ browse   Enter select   Esc cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        inner,
    );
}

pub(super) fn draw_extreme_confirmation(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let popup = centered_rect(58, 34, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Confirm Extreme effort ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .style(Style::default().bg(Color::Rgb(34, 28, 29)));
    let body = Paragraph::new(vec![
        Line::from("Extreme can consume substantially more tokens and cost more."),
        Line::from("Dynamic workflows are not implemented in this early build."),
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
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true })
    .block(block);
    frame.render_widget(body, popup);
}

#[cfg(test)]
mod tests {
    use crate::tui::render::draw;
    use crate::tui::state::{App, PrivacyPrompt};
    use crate::{Settings, provider};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn privacy_dialog_offers_separate_image_consent() {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(Settings::default());
        app.trust_prompt = false;
        app.privacy_confirmation = Some(PrivacyPrompt {
            risk: "Google/Gemini".to_owned(),
            allow_images: false,
        });
        app.pending_privacy_message = Some(provider::ChatMessage::user_with_images(
            "image request".to_owned(),
            "look".to_owned(),
            vec![
                serde_json::json!({"type":"image_url", "image_url":{"url":"data:image/png;base64,aGVsbG8="}}),
            ],
        ));
        terminal
            .draw(|frame| draw(frame, &app, 0))
            .expect("draw privacy prompt");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Allow image contents"));
        assert!(rendered.contains("enable this option"));
    }
}
