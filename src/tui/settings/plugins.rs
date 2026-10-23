//! Settings > Plugins: installed plugins (on/off, remove) and mods (approve, on/off).

use crate::extensions::mcp::{self, ServerInfo};
use crate::extensions::mcp_hub::hub;
use crate::extensions::mods::{self, Approval, ModInfo};
use crate::extensions::plugins::{self, Plugin};
use crate::tui::settings::{Focus, SettingsView};
use crate::tui::state::App;
use crate::write_settings;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

/// A question waiting for y or n.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum PluginConfirm {
    Remove(String),
    Approve(String),
    ApproveMcp(String),
}

/// Typing the values of an MCP server's secret variables, one after another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct SecretInput {
    pub(in crate::tui) id: String,
    pub(in crate::tui) variables: Vec<String>,
    pub(in crate::tui) index: usize,
    pub(in crate::tui) value: String,
}

impl SecretInput {
    pub(in crate::tui) fn new(info: &crate::extensions::mcp::ServerInfo) -> SecretInput {
        SecretInput {
            id: info.id.clone(),
            variables: info.manifest.env.clone(),
            index: 0,
            value: String::new(),
        }
    }
}

/// One selectable row.
enum Row {
    Plugin(Plugin),
    Mod(ModInfo),
    Mcp(ServerInfo),
}

impl App {
    /// The installed plugins, then the mods (of enabled plugins and from ~/.coolcode/mods),
    /// and a warning for anything that could not be read.
    fn plugin_rows(&self) -> (Vec<Row>, Vec<String>) {
        let (installed, mut warnings) = plugins::installed(&self.extensions.dirs, &self.settings);
        let (found, mod_warnings) = mods::discover(&self.extensions.dirs, &self.settings);
        warnings.extend(mod_warnings);
        let (servers, server_warnings) = self.mcp_servers();
        warnings.extend(server_warnings);
        let rows = installed
            .into_iter()
            .map(Row::Plugin)
            .chain(found.into_iter().map(Row::Mod))
            .chain(servers.into_iter().map(Row::Mcp))
            .collect();
        (rows, warnings)
    }

    pub(super) fn handle_plugins_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let (rows, _) = self.plugin_rows();
        let Some(view) = self.settings_view.as_mut() else {
            return Ok(());
        };
        if let Some(input) = view.mcp_secret.take() {
            return self.handle_secret_key(input, key);
        }
        if let Some(confirm) = view.plugin_confirm.take() {
            if !matches!(key.code, KeyCode::Char('y' | 'Y')) {
                self.notice = "Nothing changed.".to_owned();
                return Ok(());
            }
            return self.confirm_plugin_change(confirm, &rows);
        }
        let selected = view.row.min(rows.len().saturating_sub(1));
        match key.code {
            KeyCode::Up => view.row = selected.saturating_sub(1),
            KeyCode::Down => view.row = (selected + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char(' ') => match rows.get(selected) {
                Some(Row::Plugin(plugin)) => {
                    let name = plugin.manifest.name.clone();
                    if plugin.enabled {
                        self.settings.disabled_plugins.push(name.clone());
                    } else {
                        self.settings
                            .disabled_plugins
                            .retain(|other| *other != name);
                    }
                    write_settings(&self.settings)?;
                    self.forget_extension_items();
                    self.sync_mods();
                    self.notice = format!(
                        "The plugin {name} is {}.",
                        if plugin.enabled { "off" } else { "on" }
                    );
                }
                Some(Row::Mod(info)) => match mods::approval(&self.settings, info) {
                    Approval::Approved => {
                        let id = info.id.clone();
                        let off = !self.settings.disabled_mods.contains(&id);
                        if off {
                            self.settings.disabled_mods.push(id.clone());
                        } else {
                            self.settings.disabled_mods.retain(|other| *other != id);
                        }
                        write_settings(&self.settings)?;
                        self.sync_mods();
                        self.notice =
                            format!("The mod {id} is {}.", if off { "off" } else { "on" });
                    }
                    Approval::NotApproved | Approval::Changed => {
                        view.plugin_confirm = Some(PluginConfirm::Approve(info.id.clone()));
                    }
                },
                Some(Row::Mcp(info)) => match mcp::approval(&self.settings, info) {
                    Approval::Approved => {
                        let id = info.id.clone();
                        let off = !self.settings.disabled_mcp.contains(&id);
                        if off {
                            self.settings.disabled_mcp.push(id.clone());
                        } else {
                            self.settings.disabled_mcp.retain(|other| *other != id);
                        }
                        write_settings(&self.settings)?;
                        self.sync_mcp();
                        self.notice =
                            format!("The MCP server {id} is {}.", if off { "off" } else { "on" });
                    }
                    Approval::NotApproved | Approval::Changed => {
                        view.plugin_confirm = Some(PluginConfirm::ApproveMcp(info.id.clone()));
                    }
                },
                None => {}
            },
            KeyCode::Char('r') => {
                if let Some(Row::Mcp(info)) = rows.get(selected) {
                    if mcp::should_run(&self.settings, info) {
                        let id = info.id.clone();
                        self.restart_mcp(&id);
                        self.notice = format!("Restarting the MCP server {id}.");
                    } else {
                        self.notice =
                            "Only an approved server that is on can be restarted.".to_owned();
                    }
                }
            }
            KeyCode::Char('s') => {
                if let Some(Row::Mcp(info)) = rows.get(selected) {
                    if info.manifest.env.is_empty() {
                        self.notice = format!(
                            "The MCP server {} asks for no secrets (its entry has no env list).",
                            info.id
                        );
                    } else {
                        view.mcp_secret = Some(SecretInput::new(info));
                    }
                }
            }
            KeyCode::Char('x') => {
                if let Some(Row::Plugin(plugin)) = rows.get(selected) {
                    view.plugin_confirm = Some(PluginConfirm::Remove(plugin.manifest.name.clone()));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Keys while typing the value of an MCP server's secret variable.
    fn handle_secret_key(&mut self, mut input: SecretInput, key: event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.notice = "Nothing changed.".to_owned();
                return Ok(());
            }
            KeyCode::Enter => {
                let variable = input.variables[input.index].clone();
                if !input.value.is_empty() {
                    if let Err(error) = mcp::store_variable(&input.id, &variable, &input.value) {
                        self.notice = format!("Could not save the value: {error:#}");
                        return Ok(());
                    }
                    self.notice = format!("Saved {variable} in the OS credential store.");
                }
                input.value.clear();
                input.index += 1;
                if input.index >= input.variables.len() {
                    // New values only reach a server when it starts.
                    let id = input.id.clone();
                    self.restart_mcp(&id);
                    return Ok(());
                }
            }
            _ => crate::tui::state::edit_string(&mut input.value, key),
        }
        if let Some(view) = self.settings_view.as_mut() {
            view.mcp_secret = Some(input);
        }
        Ok(())
    }

    fn confirm_plugin_change(&mut self, confirm: PluginConfirm, rows: &[Row]) -> Result<()> {
        match confirm {
            PluginConfirm::Remove(name) => {
                let secrets = self.plugin_secrets(&name);
                match plugins::remove(&self.extensions.dirs, &name) {
                    Ok(()) => {
                        crate::tui::mcp::delete_secrets(&secrets);
                        self.forget_plugin(&name);
                        write_settings(&self.settings)?;
                        self.sync_mods();
                        if let Some(view) = self.settings_view.as_mut() {
                            view.row = view.row.saturating_sub(1);
                        }
                        self.notice = format!("Removed the plugin {name}.");
                    }
                    Err(error) => self.notice = format!("Could not remove the plugin: {error:#}"),
                }
            }
            PluginConfirm::ApproveMcp(id) => {
                // The entry is read again, so what is approved is what was on screen.
                let Some(info) = rows.iter().find_map(|row| match row {
                    Row::Mcp(info) if info.id == id => Some(info),
                    _ => None,
                }) else {
                    self.notice = format!("The MCP server {id} is gone.");
                    return Ok(());
                };
                mcp::approve(&mut self.settings, info);
                write_settings(&self.settings)?;
                self.sync_mcp();
                self.notice = format!("The MCP server {id} is approved and on.");
            }
            PluginConfirm::Approve(id) => {
                // The manifest is read again, so what is approved is what was on screen.
                let Some(info) = rows.iter().find_map(|row| match row {
                    Row::Mod(info) if info.id == id => Some(info),
                    _ => None,
                }) else {
                    self.notice = format!("The mod {id} is gone.");
                    return Ok(());
                };
                mods::approve(&mut self.settings, info);
                write_settings(&self.settings)?;
                self.sync_mods();
                self.notice = format!("The mod {id} is approved and on.");
            }
        }
        Ok(())
    }
}

/// How a mod stands, as shown in the list.
fn mod_state(app: &App, info: &ModInfo) -> (&'static str, Color) {
    match mods::approval(&app.settings, info) {
        Approval::Approved if app.settings.disabled_mods.contains(&info.id) => ("off", Color::Gray),
        Approval::Approved => ("on", Color::Rgb(110, 220, 130)),
        Approval::NotApproved => ("needs approval", Color::Rgb(255, 197, 92)),
        Approval::Changed => ("changed, approve again", Color::Rgb(255, 197, 92)),
    }
}

pub(super) fn draw_plugins(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    view: &SettingsView,
) {
    let (rows, warnings) = app.plugin_rows();
    let dim = Style::default().fg(Color::DarkGray);
    let heading = |text: &str| {
        Line::from(Span::styled(
            text.to_owned(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let mut lines = Vec::new();
    if let Some(input) = &view.mcp_secret {
        let variable = &input.variables[input.index.min(input.variables.len() - 1)];
        lines.push(heading(&format!("Secrets of the MCP server {}", input.id)));
        lines.push(Line::from(Span::styled(
            format!(
                "Variable {} of {}: {variable}. Kept in the OS credential store, never in a file.",
                input.index + 1,
                input.variables.len()
            ),
            dim,
        )));
        lines.push(Line::from(""));
        let mut field = vec![Span::styled(
            "▏",
            Style::default().fg(crate::tui::theme::accent()),
        )];
        if input.value.is_empty() {
            let saved = if mcp::has_variable(&input.id, variable) {
                "•••••••• saved · type to replace · Enter keeps it"
            } else {
                "paste the value here"
            };
            field.push(Span::styled(saved, dim));
        } else {
            field.push(Span::styled(
                "•".repeat(input.value.chars().count().min(42)),
                Style::default().fg(Color::Rgb(185, 195, 205)),
            ));
            field.push(Span::styled(
                "▏",
                Style::default().fg(crate::tui::theme::accent()),
            ));
        }
        lines.push(Line::from(field).style(Style::default().bg(crate::tui::theme::input())));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter saves and goes on · Esc cancels",
            dim,
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        return;
    }
    if let Some(confirm) = &view.plugin_confirm {
        match confirm {
            PluginConfirm::Remove(name) => {
                lines.push(heading(&format!(
                    "Remove the plugin {name} and delete its folder?"
                )));
                lines.push(Line::from(Span::styled(
                    "Its skills, commands and mods go with it.",
                    dim,
                )));
            }
            PluginConfirm::ApproveMcp(id) => {
                let info = rows.iter().find_map(|row| match row {
                    Row::Mcp(info) if info.id == *id => Some(info),
                    _ => None,
                });
                if let Some(info) = info {
                    if mcp::approval(&app.settings, info) == Approval::Changed {
                        lines.push(Line::from(Span::styled(
                            "This MCP server changed since you approved it.",
                            Style::default().fg(Color::Rgb(255, 197, 92)),
                        )));
                    }
                    lines.push(heading("Run this MCP server?"));
                    lines.push(Line::from(""));
                    for (label, value) in [
                        ("Server", info.id.clone()),
                        ("From", info.source.label()),
                        ("Command", info.manifest.command_line()),
                        ("Runs in", info.dir.display().to_string()),
                        (
                            "Secrets",
                            if info.manifest.env.is_empty() {
                                "none".to_owned()
                            } else {
                                info.manifest.env.join(", ")
                            },
                        ),
                    ] {
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("  {label:<10}"),
                                Style::default().fg(Color::Gray),
                            ),
                            Span::styled(value, Style::default().fg(Color::White)),
                        ]));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "It starts now and with every session and adds tools the model may call. Every call asks you first (Auto mode: its guard models decide), and its answers reach the model as data, never as instructions. Approve only programs you trust.",
                        dim,
                    )));
                }
            }
            PluginConfirm::Approve(id) => {
                let info = rows.iter().find_map(|row| match row {
                    Row::Mod(info) if info.id == *id => Some(info),
                    _ => None,
                });
                if let Some(info) = info {
                    if mods::approval(&app.settings, info) == Approval::Changed {
                        lines.push(Line::from(Span::styled(
                            "This mod changed since you approved it.",
                            Style::default().fg(Color::Rgb(255, 197, 92)),
                        )));
                    }
                    lines.push(heading("Run this mod?"));
                    lines.push(Line::from(""));
                    for (label, value) in [
                        ("Mod", info.id.clone()),
                        ("Command", info.manifest.command_line()),
                        ("Runs in", info.dir.display().to_string()),
                        (
                            "Receives",
                            if info.manifest.events.is_empty() {
                                "no events".to_owned()
                            } else {
                                info.manifest.events.join(", ")
                            },
                        ),
                    ] {
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("  {label:<10}"),
                                Style::default().fg(Color::Gray),
                            ),
                            Span::styled(value, Style::default().fg(Color::White)),
                        ]));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "It starts now and with every session, and can show a status and notices. It cannot approve, block or change anything.",
                        dim,
                    )));
                }
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("y yes · n no", dim)));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        return;
    }
    let focused = view.focus == Focus::Content;
    let selected = view.row.min(rows.len().saturating_sub(1));
    let marker = |index: usize| {
        Span::styled(
            if focused && index == selected {
                "▸ "
            } else {
                "  "
            },
            Style::default().fg(crate::tui::theme::accent()),
        )
    };
    lines.push(heading("Plugins"));
    let plugin_count = rows
        .iter()
        .filter(|row| matches!(row, Row::Plugin(_)))
        .count();
    if plugin_count == 0 {
        lines.push(Line::from(Span::styled(
            "  No plugins are installed. Install one with /plugin install <git-url or folder>.",
            dim,
        )));
    }
    let mod_count = rows.iter().filter(|row| matches!(row, Row::Mod(_))).count();
    let statuses = hub().statuses();
    for (index, row) in rows.iter().enumerate() {
        if index == plugin_count {
            lines.push(Line::from(""));
            lines.push(heading("Mods"));
        }
        if index == plugin_count + mod_count {
            if mod_count == 0 {
                lines.push(Line::from(Span::styled(
                    "  No mods. A mod comes with a plugin or lives in ~/.coolcode/mods/<name>/mod.toml.",
                    dim,
                )));
            }
            lines.push(Line::from(""));
            lines.push(heading("MCP servers"));
        }
        lines.push(match row {
            Row::Plugin(plugin) => Line::from(vec![
                marker(index),
                Span::styled(
                    format!(
                        "{:<26}",
                        format!("{} {}", plugin.manifest.name, plugin.manifest.version)
                    ),
                    Style::default().fg(Color::White),
                ),
                if plugin.enabled {
                    Span::styled("on   ", Style::default().fg(Color::Rgb(110, 220, 130)))
                } else {
                    Span::styled("off  ", Style::default().fg(Color::Gray))
                },
                Span::styled(plugin.manifest.description.clone(), dim),
            ]),
            Row::Mcp(info) => {
                let (state, color) = crate::tui::mcp::mcp_state(app, info, &statuses);
                Line::from(vec![
                    marker(index),
                    Span::styled(
                        format!("{:<26}", info.id),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(format!("{state} "), Style::default().fg(color)),
                    Span::styled(format!(" {}", info.manifest.command_line()), dim),
                ])
            }
            Row::Mod(info) => {
                let (state, color) = mod_state(app, info);
                Line::from(vec![
                    marker(index),
                    Span::styled(
                        format!("{:<26}", info.id),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(format!("{state} "), Style::default().fg(color)),
                    Span::styled(format!(" {}", info.manifest.command_line()), dim),
                ])
            }
        });
    }
    if rows.len() == plugin_count {
        lines.push(Line::from(""));
        lines.push(heading("Mods"));
        lines.push(Line::from(Span::styled(
            "  No mods. A mod comes with a plugin or lives in ~/.coolcode/mods/<name>/mod.toml.",
            dim,
        )));
    }
    if rows.len() <= plugin_count + mod_count {
        lines.push(Line::from(""));
        lines.push(heading("MCP servers"));
        lines.push(Line::from(Span::styled(
            "  None. Add one with /mcp add <name> <command>, or list it in ~/.coolcode/mcp.toml.",
            dim,
        )));
    }
    for warning in warnings {
        lines.push(Line::from(Span::styled(
            format!("Skipped {warning}"),
            Style::default().fg(Color::Rgb(255, 197, 92)),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

pub(super) fn plugins_hint(view: &SettingsView) -> &'static str {
    if view.mcp_secret.is_some() {
        "Enter save   Esc cancel"
    } else if view.plugin_confirm.is_some() {
        "y confirm   n cancel"
    } else {
        "↑↓ move   Enter switch on/off or approve   s MCP secrets   r restart   x remove plugin   ← sections   Esc back"
    }
}

#[cfg(test)]
mod tests {
    use crate::extensions::Dirs;
    use crate::tui::settings::Section;
    use crate::tui::state::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn home() -> PathBuf {
        let home = std::env::temp_dir().join(format!(
            "coolcode-settings-plugins-{}",
            uuid::Uuid::new_v4()
        ));
        let plugin = home.join(".coolcode/plugins/kit");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.toml"),
            "name = \"kit\"\nversion = \"0.3.0\"\ndescription = \"A kit\"\n[[mods]]\nname = \"watch\"\ncommand = \"node\"\nargs = [\"watch.js\"]\nevents = [\"turn_finished\"]\n",
        )
        .unwrap();
        home
    }

    fn app(home: &std::path::Path) -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.extensions.dirs = Dirs::under(home);
        app.open_settings(Section::Plugins);
        press(&mut app, KeyCode::Right);
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_view_key(KeyEvent::new(code, KeyModifiers::NONE))
            .expect("key");
    }

    fn screen(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 32)).expect("terminal");
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
    fn plugins_and_their_mods_are_listed_with_their_state() {
        let home = home();
        let app = app(&home);
        let shown = screen(&app);
        for expected in [
            "Plugins",
            "kit 0.3.0",
            "A kit",
            "kit/watch",
            "needs approval",
            "node watch.js",
        ] {
            assert!(shown.contains(expected), "{expected}:\n{shown}");
        }
        let empty = self::app(
            &std::env::temp_dir().join(format!("coolcode-none-{}", uuid::Uuid::new_v4())),
        );
        assert!(screen(&empty).contains("No plugins"), "{}", screen(&empty));
    }

    #[test]
    fn a_plugin_is_switched_off_and_on_and_its_mods_go_with_it() {
        let home = home();
        let mut app = app(&home);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.disabled_plugins, ["kit"]);
        let shown = screen(&app);
        assert!(
            !shown.contains("kit/watch"),
            "a disabled plugin's mods are hidden:\n{shown}"
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.disabled_plugins.is_empty());
    }

    #[test]
    fn a_mod_runs_only_after_the_user_saw_its_command_and_said_yes() {
        let home = home();
        let mut app = app(&home);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let question = screen(&app);
        assert!(question.contains("Run this mod?"), "{question}");
        assert!(
            question.contains("node watch.js") && question.contains("turn_finished"),
            "{question}"
        );
        press(&mut app, KeyCode::Char('n'));
        assert!(app.settings.approved_mods.is_empty());
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.settings.approved_mods.contains_key("kit/watch"));
        assert!(screen(&app).contains(" on "), "{}", screen(&app));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.disabled_mods, ["kit/watch"], "switched off");
        press(&mut app, KeyCode::Enter);
        assert!(
            app.settings.disabled_mods.is_empty(),
            "and on again without asking"
        );
        // A changed manifest asks again.
        let manifest = home.join(".coolcode/plugins/kit/plugin.toml");
        let text = std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("watch.js", "other.js");
        std::fs::write(&manifest, text).unwrap();
        assert!(screen(&app).contains("changed"), "{}", screen(&app));
        press(&mut app, KeyCode::Enter);
        assert!(screen(&app).contains("node other.js"), "{}", screen(&app));
    }

    /// An app whose user file lists the test MCP server as `files` with a secret variable.
    fn mcp_app(home: &std::path::Path) -> App {
        crate::extensions::mcp_hub::use_private_hub();
        let mut app = app(home);
        app.workspace_trusted = true;
        app.extensions.mods_active = true;
        let mut manifest = crate::extensions::mcp_client::tests::helper("basic").manifest;
        manifest.name = "files".to_owned();
        manifest.env = vec!["TOKEN".to_owned()];
        crate::extensions::mcp::add_user_server(&app.extensions.dirs, manifest).unwrap();
        app
    }

    /// Moves the cursor onto the `files` row (after the plugin `kit` and its mod).
    fn select_files(app: &mut App) {
        for _ in 0..2 {
            press(app, KeyCode::Down);
        }
    }

    fn wait_for_ready(count: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while crate::extensions::mcp_hub::hub().ready() != count {
            assert!(
                std::time::Instant::now() < deadline,
                "servers did not get ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn mcp_servers_are_listed_and_run_only_after_the_user_saw_the_command_and_said_yes() {
        let home = home();
        let mut app = mcp_app(&home);
        let shown = screen(&app);
        for expected in ["MCP servers", "files", "needs approval"] {
            assert!(shown.contains(expected), "{expected}:\n{shown}");
        }
        select_files(&mut app);
        press(&mut app, KeyCode::Enter);
        let question = screen(&app);
        for expected in [
            "Run this MCP server?",
            "your mcp.toml",
            "TOKEN",
            "Approve only",
            "mcp_helper_process",
        ] {
            assert!(question.contains(expected), "{expected}:\n{question}");
        }
        press(&mut app, KeyCode::Char('n'));
        assert!(app.settings.approved_mcp.is_empty());
        assert!(crate::extensions::mcp_hub::hub().statuses().is_empty());
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.settings.approved_mcp.contains_key("files"));
        wait_for_ready(1);
        assert!(
            screen(&app).contains("running, 7 tools"),
            "{}",
            screen(&app)
        );
        // Switch it off and on without being asked again.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.disabled_mcp, ["files"]);
        assert!(crate::extensions::mcp_hub::hub().statuses().is_empty());
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.disabled_mcp.is_empty());
        wait_for_ready(1);
        app.stop_mcp();
    }

    #[test]
    fn secrets_are_typed_hidden_stored_in_the_credential_store_and_never_in_settings() {
        let home = home();
        let mut app = mcp_app(&home);
        select_files(&mut app);
        press(&mut app, KeyCode::Char('s'));
        let empty = screen(&app);
        assert!(empty.contains("Secrets of the MCP server files"), "{empty}");
        assert!(empty.contains("paste the value here"), "{empty}");
        for character in "tok-123".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        let typed = screen(&app);
        assert!(!typed.contains("tok-123"), "the value is hidden:\n{typed}");
        assert!(typed.contains("•••••••"), "{typed}");
        press(&mut app, KeyCode::Enter);
        assert!(app.settings_view.as_ref().unwrap().mcp_secret.is_none());
        assert!(crate::extensions::mcp::has_variable("files", "TOKEN"));
        assert_eq!(
            crate::extensions::mcp::variables(
                &app.mcp_servers()
                    .0
                    .into_iter()
                    .find(|s| s.id == "files")
                    .unwrap()
            ),
            [("TOKEN".to_owned(), "tok-123".to_owned())]
        );
        let on_disk = toml::to_string(&app.settings).unwrap();
        assert!(!on_disk.contains("tok-123"), "{on_disk}");
        // Esc leaves everything as it was.
        press(&mut app, KeyCode::Char('s'));
        assert!(screen(&app).contains("saved"), "{}", screen(&app));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        assert!(crate::extensions::mcp::has_variable("files", "TOKEN"));
        let _ = crate::extensions::mcp::delete_variable("files", "TOKEN");
    }

    #[test]
    fn removing_a_plugin_asks_first_then_deletes_it() {
        let home = home();
        let mut app = app(&home);
        app.settings
            .approved_mods
            .insert("kit/watch".to_owned(), "x".to_owned());
        press(&mut app, KeyCode::Char('x'));
        assert!(
            screen(&app).contains("Remove the plugin kit"),
            "{}",
            screen(&app)
        );
        press(&mut app, KeyCode::Esc);
        assert!(home.join(".coolcode/plugins/kit").exists());
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert!(!home.join(".coolcode/plugins/kit").exists());
        assert!(app.settings.approved_mods.is_empty());
        assert!(app.notice.contains("Removed"), "{}", app.notice);
    }
}
