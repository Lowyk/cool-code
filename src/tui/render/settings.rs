use crate::Settings;
use crate::tui::effort::effort_name;
use crate::tui::forms::provider_focus_layout;
use crate::tui::models::{available_chain_models, model_display_for_profile};
use crate::tui::render::centered_rect;
use crate::tui::state::{App, ChainDraft, PROVIDER_PRESETS, SettingsTab};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(super) fn draw_settings(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let popup = centered_rect(82, 78, area);
    frame.render_widget(Clear, popup);
    let title = if app.provider_form.is_some() {
        " Add provider "
    } else if app.chain_form.is_some() {
        " Edit model chain "
    } else {
        " Settings "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(98, 213, 244)))
        .style(Style::default().bg(Color::Rgb(29, 30, 32)))
        .padding(ratatui::widgets::Padding::horizontal(2));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if let Some(form) = &app.provider_form {
        if form.choosing_preset {
            let mut lines = vec![Line::from("Choose a provider preset:"), Line::from("")];
            for (index, preset) in PROVIDER_PRESETS.iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(
                        if index == form.preset { "› " } else { "  " },
                        Style::default().fg(Color::Rgb(98, 213, 244)),
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
        let mut render_field = |label: &str, value: &str, selected: bool, masked: bool| {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        if selected { "› " } else { "  " },
                        Style::default().fg(Color::Rgb(98, 213, 244)),
                    ),
                    Span::styled(
                        label,
                        Style::default().fg(if selected { Color::White } else { Color::Gray }),
                    ),
                ])),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y = y.saturating_add(1);
            let visible = if masked {
                "•".repeat(value.chars().count().min(42))
            } else if value.is_empty() {
                "(empty)".to_owned()
            } else {
                value.to_owned()
            };
            frame.render_widget(
                Paragraph::new(visible).style(Style::default().fg(Color::Rgb(185, 195, 205))),
                Rect::new(inner.x + 3, y, inner.width.saturating_sub(3), 1),
            );
            y = y.saturating_add(2);
        };
        render_field("Alias", &form.alias, form.focus == 0, false);
        if preset.custom {
            render_field("Base URL", &form.base_url, form.focus == 1, false);
        }
        render_field(
            "API Key · kept in OS credential store",
            &form.api_key,
            form.focus == key_focus,
            true,
        );
        frame.render_widget(
            Paragraph::new("Model IDs").style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect::new(inner.x + 2, y, inner.width.saturating_sub(2), 1),
        );
        y = y.saturating_add(1);
        for (index, model) in form.models.iter().enumerate() {
            let row_focus = model_start + index * 3;
            let row = Line::from(vec![
                Span::styled(
                    if form.focus == row_focus {
                        "› "
                    } else {
                        "  "
                    },
                    Style::default().fg(Color::Rgb(98, 213, 244)),
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
                    Color::Rgb(98, 213, 244)
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
                        Color::Rgb(98, 213, 244)
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
        return;
    }

    let tabs = Line::from(vec![
        Span::styled(
            " General ",
            if app.settings_tab == SettingsTab::General {
                Style::default()
                    .fg(Color::Rgb(98, 213, 244))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
        Span::raw("   "),
        Span::styled(
            " Providers ",
            if app.settings_tab == SettingsTab::Providers {
                Style::default()
                    .fg(Color::Rgb(98, 213, 244))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
        Span::raw("   "),
        Span::styled(
            " Auto-switch models ",
            if app.settings_tab == SettingsTab::AutoSwitch {
                Style::default()
                    .fg(Color::Rgb(98, 213, 244))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
        Span::raw("   "),
        Span::styled(
            " Privacy ",
            if app.settings_tab == SettingsTab::Privacy {
                Style::default()
                    .fg(Color::Rgb(98, 213, 244))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
    ]);
    frame.render_widget(
        Paragraph::new(tabs).alignment(Alignment::Center),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    match app.settings_tab {
        SettingsTab::General => {
            let model = app.settings.model.as_deref().unwrap_or("not set");
            let provider = app
                .settings
                .provider
                .as_deref()
                .unwrap_or("openai-compatible");
            let lines = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("Provider   ", Style::default().fg(Color::Gray)),
                    Span::raw(provider),
                ]),
                Line::from(vec![
                    Span::styled("Model      ", Style::default().fg(Color::Gray)),
                    Span::raw(model),
                ]),
                Line::from(vec![
                    Span::styled("Effort     ", Style::default().fg(Color::Gray)),
                    Span::raw(effort_name(app.settings.effort)),
                ]),
                Line::from(vec![
                    Span::styled("Permissions ", Style::default().fg(Color::Gray)),
                    Span::raw(&app.settings.permission_mode),
                ]),
                Line::from(
                    "Plan: approve an exact action plan first · Accept Edits: edits auto, commands ask",
                ),
                Line::from(
                    "Accept Minimal: edits + verification-command allowlist · Auto: safe edits/checks auto",
                ),
                Line::from(
                    "Accept Everything: edits and shell commands run without per-action approval",
                ),
                Line::from(""),
                Line::from(
                    "API keys are kept in the OS credential store, never plain-text config.",
                ),
                Line::from("Use the Providers tab to add a connection and choose it for chat."),
            ];
            frame.render_widget(
                Paragraph::new(lines).wrap(Wrap { trim: true }),
                Rect::new(
                    inner.x,
                    inner.y + 2,
                    inner.width,
                    inner.height.saturating_sub(4),
                ),
            );
        }
        SettingsTab::Providers => {
            if app.settings.providers.is_empty() {
                frame.render_widget(
                    Paragraph::new("No providers added yet. Press N to add an API connection.")
                        .style(Style::default().fg(Color::Gray))
                        .alignment(Alignment::Center),
                    Rect::new(inner.x, inner.y + 3, inner.width, 2),
                );
            } else {
                for (index, profile) in app.settings.providers.iter().enumerate() {
                    let y = inner.y + 2 + index as u16 * 2;
                    let selected = index == app.provider_index;
                    let active = app.settings.active_provider_id.as_deref() == Some(&profile.id);
                    let is_default =
                        app.settings.default_provider_id.as_deref() == Some(&profile.id);
                    let indicator = if active {
                        "●"
                    } else if profile.draft {
                        "◌"
                    } else {
                        "○"
                    };
                    let model_count = profile
                        .models
                        .len()
                        .max(usize::from(!profile.model.is_empty()));
                    let status = if profile.draft {
                        "draft".to_owned()
                    } else {
                        format!(
                            "{model_count} model(s){}{}",
                            if is_default { " · default" } else { "" },
                            if profile.auto_switch { " · auto" } else { "" }
                        )
                    };
                    let line = Line::from(vec![
                        Span::styled(
                            if selected { "› " } else { "  " },
                            Style::default().fg(Color::Rgb(98, 213, 244)),
                        ),
                        Span::styled(
                            indicator,
                            Style::default().fg(if active {
                                Color::Green
                            } else {
                                Color::DarkGray
                            }),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            &profile.name,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  ·  {}  ·  {status}", profile.adapter),
                            Style::default().fg(Color::Gray),
                        ),
                    ]);
                    frame.render_widget(
                        Paragraph::new(line).wrap(Wrap { trim: true }),
                        Rect::new(inner.x, y, inner.width, 2),
                    );
                }
            }
            let help_y = inner.bottom().saturating_sub(2);
            frame.render_widget(
                Paragraph::new(
                    "N add · E edit · D delete · ↑/↓ choose · Enter default · Space auto-switch · Tab switch tab",
                )
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
                Rect::new(inner.x, help_y, inner.width, 2),
            );
        }
        SettingsTab::AutoSwitch => {
            if app.settings.model_chains.is_empty() {
                frame.render_widget(
                    Paragraph::new("No model chains yet. Press N to create a preference chain.")
                        .style(Style::default().fg(Color::Gray))
                        .alignment(Alignment::Center),
                    Rect::new(inner.x, inner.y + 3, inner.width, 2),
                );
            } else {
                for (index, chain) in app.settings.model_chains.iter().enumerate() {
                    let y = inner.y + 2 + index as u16 * 2;
                    let active = app.settings.active_chain_id.as_deref() == Some(&chain.id);
                    let line = Line::from(vec![
                        Span::styled(
                            if index == app.chain_index {
                                "› "
                            } else {
                                "  "
                            },
                            Style::default().fg(Color::Rgb(98, 213, 244)),
                        ),
                        Span::styled(
                            if active { "●" } else { "○" },
                            Style::default().fg(if active {
                                Color::Green
                            } else {
                                Color::DarkGray
                            }),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            &chain.alias,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(
                                "  ·  {}  ·  {} preferred models  ·  auto-on-select {}",
                                chain.id,
                                chain.members.len(),
                                if chain.activate_on_select {
                                    "on"
                                } else {
                                    "off"
                                }
                            ),
                            Style::default().fg(Color::Gray),
                        ),
                    ]);
                    frame.render_widget(
                        Paragraph::new(line).wrap(Wrap { trim: true }),
                        Rect::new(inner.x, y, inner.width, 2),
                    );
                }
            }
            frame.render_widget(Paragraph::new("N create · E edit · D delete · Enter activate · /chain <id> · Alt+C toggle current chain").style(Style::default().fg(Color::DarkGray)).alignment(Alignment::Center).wrap(Wrap { trim: true }), Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 2));
        }
        SettingsTab::Privacy => {
            let root = std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "(unknown folder)".to_owned());
            let lines = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("Workspace ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        if app.workspace_trusted {
                            "TRUSTED"
                        } else {
                            "UNTRUSTED"
                        },
                        Style::default().fg(if app.workspace_trusted {
                            Color::Green
                        } else {
                            Color::Rgb(255, 197, 92)
                        }),
                    ),
                ]),
                Line::from(root),
                Line::from(
                    "A trusted workspace allows COOL.md/@path reads and permission-gated exact text edits. Trust is stored only in .coolcode/trusted.",
                ),
                Line::from(""),
                Line::from(format!(
                    "Privacy acknowledgements: {}   ·   Image-content grants: {}",
                    app.settings.privacy_acknowledged.len(),
                    app.settings.privacy_image_acknowledged.len()
                )),
                Line::from(
                    "Custom local redaction values are stored in the OS credential store; manage them with /privacy add|clear.",
                ),
                Line::from(
                    "Text redaction is best-effort. Image data is sent unredacted only when explicitly allowed in the model privacy dialog.",
                ),
                Line::from(""),
                Line::from(
                    "T trust/revoke workspace   R reset privacy acknowledgements   C clear redaction values",
                ),
                Line::from(
                    "/privacy add <value> adds a local redaction value · /privacy clear clears values",
                ),
            ];
            frame.render_widget(
                Paragraph::new(lines).wrap(Wrap { trim: true }),
                Rect::new(
                    inner.x,
                    inner.y + 1,
                    inner.width,
                    inner.height.saturating_sub(3),
                ),
            );
        }
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
                    Style::default().fg(Color::Rgb(98, 213, 244)),
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
                Style::default().fg(Color::Rgb(98, 213, 244)),
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
                Style::default().fg(Color::Rgb(98, 213, 244)),
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
            Color::Rgb(98, 213, 244)
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
            Style::default().fg(Color::Rgb(98, 213, 244)),
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
