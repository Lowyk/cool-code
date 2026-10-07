use crate::Settings;
use crate::tui::forms::{has_endpoint_fields, provider_focus_layout};
use crate::tui::models::{available_chain_models, model_display_for_profile};
use crate::tui::state::{App, ChainDraft, PROVIDER_PRESETS};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub(in crate::tui) fn draw_open_form(frame: &mut ratatui::Frame<'_>, inner: Rect, app: &App) {
    if let Some(form) = &app.provider_form {
        if form.choosing_preset {
            let mut lines = vec![Line::from("Choose a provider preset:"), Line::from("")];
            for (index, preset) in PROVIDER_PRESETS.iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(
                        if index == form.preset { "› " } else { "  " },
                        Style::default().fg(crate::tui::theme::accent()),
                    ),
                    Span::styled(
                        preset.label,
                        Style::default().fg(if index == form.preset {
                            Color::White
                        } else {
                            Color::Gray
                        }),
                    ),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "↑/↓ choose · Enter continue · Esc cancel",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
            return;
        }
        let preset = &PROVIDER_PRESETS[form.preset];
        let (key_focus, model_start, create_focus, save_focus, draft_focus, cancel_focus) =
            provider_focus_layout(form);
        let mut y = inner.y;
        let mut render_field =
            |label: &str, value: &str, placeholder: &str, selected: bool, masked: bool| {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            if selected { "› " } else { "  " },
                            Style::default().fg(crate::tui::theme::accent()),
                        ),
                        Span::styled(
                            label,
                            Style::default().fg(if selected { Color::White } else { Color::Gray }),
                        ),
                    ])),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                y = y.saturating_add(1);
                let placeholder_shown = value.is_empty() && !masked;
                let visible = if masked {
                    "•".repeat(value.chars().count().min(42))
                } else if value.is_empty() {
                    if placeholder.is_empty() {
                        "(empty)".to_owned()
                    } else {
                        placeholder.to_owned()
                    }
                } else {
                    value.to_owned()
                };
                let color = if placeholder_shown {
                    Color::DarkGray
                } else {
                    Color::Rgb(185, 195, 205)
                };
                frame.render_widget(
                    Paragraph::new(visible).style(Style::default().fg(color)),
                    Rect::new(inner.x + 3, y, inner.width.saturating_sub(3), 1),
                );
                y = y.saturating_add(2);
            };
        // An empty alias shows the suggested name dimly; typing replaces it, leaving it blank uses it.
        render_field(
            "Alias",
            &form.alias,
            &form.suggested_alias,
            form.focus == 0,
            false,
        );
        if preset.custom {
            render_field("Base URL", &form.base_url, "", form.focus == 1, false);
        }
        let key_label = match (form.existing_id.is_some(), preset.key_prefix) {
            _ if preset.key_optional => "API Key · optional, local servers need none".to_owned(),
            (true, Some(prefix)) => {
                format!("API Key · starts with {prefix} · leave blank to keep the saved key")
            }
            (true, None) => "API Key · leave blank to keep the saved key".to_owned(),
            (false, Some(prefix)) => {
                format!("API Key · starts with {prefix} · kept in OS credential store")
            }
            (false, None) => "API Key · kept in OS credential store".to_owned(),
        };
        render_field(&key_label, &form.api_key, "", form.focus == key_focus, true);
        if has_endpoint_fields(form) {
            render_field(
                "Models endpoint · optional",
                &form.models_endpoint,
                "path or URL on the same host, e.g. models",
                form.focus == key_focus + 1,
                false,
            );
            render_field(
                "Limits endpoint · optional",
                &form.limits_endpoint,
                "path or URL on the same host, e.g. subscription/limits",
                form.focus == key_focus + 2,
                false,
            );
        }
        let managed_count = form
            .existing_id
            .as_deref()
            .and_then(|id| {
                app.settings
                    .providers
                    .iter()
                    .find(|profile| profile.id == id)
            })
            .map_or(0, |profile| profile.models.len());
        if form.managed_models {
            frame.render_widget(
                Paragraph::new(format!(
                    "{managed_count} models come from the models endpoint. Manage them in Settings → Models; press f in Providers to refresh."
                ))
                .style(Style::default().fg(Color::Gray))
                .wrap(Wrap { trim: true }),
                Rect::new(inner.x + 2, y, inner.width.saturating_sub(2), 3),
            );
            y = y.saturating_add(3);
        }
        if !form.managed_models {
            frame.render_widget(
                Paragraph::new("Model IDs").style(
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Rect::new(inner.x + 2, y, inner.width.saturating_sub(2), 1),
            );
            y = y.saturating_add(1);
        }
        for (index, model) in form.models.iter().enumerate() {
            let row_focus = model_start + index * 3;
            let row = Line::from(vec![
                Span::styled(
                    if form.focus == row_focus {
                        "› "
                    } else {
                        "  "
                    },
                    Style::default().fg(crate::tui::theme::accent()),
                ),
                Span::styled(
                    if model.id.is_empty() {
                        "model-id"
                    } else {
                        &model.id
                    },
                    Style::default().fg(if form.focus == row_focus {
                        Color::White
                    } else {
                        Color::Gray
                    }),
                ),
                Span::styled("   →   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if model.name.is_empty() {
                        "(auto name)"
                    } else {
                        &model.name
                    },
                    Style::default().fg(if form.focus == row_focus + 1 {
                        Color::White
                    } else {
                        Color::Gray
                    }),
                ),
                Span::styled("   ", Style::default()),
                Span::styled(
                    "[X]",
                    Style::default().fg(if form.focus == row_focus + 2 {
                        Color::Red
                    } else {
                        Color::DarkGray
                    }),
                ),
            ]);
            frame.render_widget(
                Paragraph::new(row).wrap(Wrap { trim: true }),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y = y.saturating_add(1);
        }
        let buttons_y = inner.bottom().saturating_sub(3);
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                if form.focus == create_focus {
                    "› Create model"
                } else {
                    "  Create model"
                },
                Style::default().fg(if form.focus == create_focus {
                    crate::tui::theme::accent()
                } else {
                    Color::Gray
                }),
            )])),
            Rect::new(inner.x, y, inner.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    if form.focus == save_focus {
                        "[ Save ]"
                    } else {
                        " Save "
                    },
                    Style::default().fg(if form.focus == save_focus {
                        Color::Green
                    } else {
                        Color::Gray
                    }),
                ),
                Span::raw("   "),
                Span::styled(
                    if form.focus == draft_focus {
                        "[ Draft ]"
                    } else {
                        " Draft "
                    },
                    Style::default().fg(if form.focus == draft_focus {
                        crate::tui::theme::accent()
                    } else {
                        Color::Gray
                    }),
                ),
                Span::raw("   "),
                Span::styled(
                    if form.focus == cancel_focus {
                        "[ Cancel ]"
                    } else {
                        " Cancel "
                    },
                    Style::default().fg(if form.focus == cancel_focus {
                        Color::Red
                    } else {
                        Color::Gray
                    }),
                ),
            ]))
            .alignment(Alignment::Center),
            Rect::new(inner.x, buttons_y, inner.width, 1),
        );
        frame.render_widget(
            Paragraph::new("Tab/↓ next · ↑ previous · Enter activate · Esc cancel")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
        );
        return;
    }
    if let Some(form) = &app.chain_form {
        draw_chain_form(frame, inner, form, &app.settings);
    }
}

pub(super) fn draw_chain_form(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    form: &ChainDraft,
    settings: &Settings,
) {
    if form.picking_member {
        let models = available_chain_models(settings);
        let mut lines = vec![
            Line::from("Choose a configured provider model:"),
            Line::from(""),
        ];
        for (index, (member, label)) in models.iter().enumerate() {
            let selected_already = form.members.iter().any(|existing| {
                existing.provider_id == member.provider_id && existing.model_id == member.model_id
            });
            lines.push(Line::from(vec![
                Span::styled(
                    if index == form.candidate_index {
                        "› "
                    } else {
                        "  "
                    },
                    Style::default().fg(crate::tui::theme::accent()),
                ),
                Span::styled(
                    if selected_already { "✓ " } else { "  " },
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    label,
                    Style::default().fg(if index == form.candidate_index {
                        Color::White
                    } else {
                        Color::Gray
                    }),
                ),
                Span::styled(
                    format!("  ·  {}", member.model_id),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "↑/↓ browse · Enter add · Esc back",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
        return;
    }
    let mut lines = vec![
        chain_field_line("Alias", &form.alias, form.focus == 0),
        chain_field_line("ID", &form.id, form.focus == 1),
        Line::from(vec![
            Span::styled(
                if form.focus == 2 { "› " } else { "  " },
                Style::default().fg(crate::tui::theme::accent()),
            ),
            Span::styled(
                format!(
                    "Auto-enable chain when selecting a member model: {} (Space)",
                    if form.activate_on_select { "ON" } else { "OFF" }
                ),
                Style::default().fg(if form.focus == 2 {
                    Color::White
                } else {
                    Color::Gray
                }),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Preferred model order · left/right moves selected model",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    for (index, member) in form.members.iter().enumerate() {
        let provider_name = settings
            .providers
            .iter()
            .find(|profile| profile.id == member.provider_id)
            .map(|profile| profile.name.as_str())
            .unwrap_or("missing provider");
        let display = model_display_for_profile(settings, &member.provider_id, &member.model_id);
        lines.push(Line::from(vec![
            Span::styled(
                if form.focus == 3 && form.member_index == index {
                    "› "
                } else {
                    "  "
                },
                Style::default().fg(crate::tui::theme::accent()),
            ),
            Span::styled(
                format!("{}. {}", index + 1, display),
                Style::default().fg(if form.focus == 3 && form.member_index == index {
                    Color::White
                } else {
                    Color::Gray
                }),
            ),
            Span::styled(
                format!("  ·  {provider_name}  ·  {}  ·  [x]", member.model_id),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::from(Span::styled(
        if form.focus == 3 {
            "› A · add model"
        } else {
            "  A · add model"
        },
        Style::default().fg(if form.focus == 3 {
            crate::tui::theme::accent()
        } else {
            Color::Gray
        }),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            if form.focus == 4 {
                "[ Save ]"
            } else {
                " Save "
            },
            Style::default().fg(if form.focus == 4 {
                Color::Green
            } else {
                Color::Gray
            }),
        ),
        Span::raw("   "),
        Span::styled(
            if form.focus == 5 {
                "[ Cancel ]"
            } else {
                " Cancel "
            },
            Style::default().fg(if form.focus == 5 {
                Color::Red
            } else {
                Color::Gray
            }),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "Tab fields · A add · ↑/↓ select priority · ←/→ reorder · X remove · Esc cancel",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

pub(super) fn chain_field_line<'a>(label: &'a str, value: &'a str, selected: bool) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            if selected { "› " } else { "  " },
            Style::default().fg(crate::tui::theme::accent()),
        ),
        Span::styled(
            label,
            Style::default().fg(if selected { Color::White } else { Color::Gray }),
        ),
        Span::raw("   "),
        Span::styled(
            if value.is_empty() {
                "(type here)"
            } else {
                value
            },
            Style::default().fg(Color::Rgb(185, 195, 205)),
        ),
    ])
}
