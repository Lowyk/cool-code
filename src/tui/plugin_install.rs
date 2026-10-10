//! `/plugin install | list | remove`, and the review shown before an install is kept.

use crate::extensions::plugins::{self, Staged};
use crate::tui::render::centered_rect;
use crate::tui::state::App;
use crate::write_settings;
use anyhow::Result;
use crossterm::event::{self, KeyCode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::sync::mpsc::Receiver;

/// An install in progress.
pub(in crate::tui) enum Install {
    /// Cloning or copying on a background thread.
    Fetching(Receiver<std::result::Result<Staged, String>>),
    /// Waiting for the user to say yes or no.
    Review {
        staged: Box<Staged>,
        lines: Vec<String>,
        scroll: u16,
    },
}

const USAGE: &str =
    "Usage: /plugin install <git-url or folder> | /plugin list | /plugin remove <name>";

impl App {
    /// Whether the install review has the keyboard.
    pub(in crate::tui) fn plugin_review_open(&self) -> bool {
        matches!(self.extensions.install, Some(Install::Review { .. }))
    }

    /// `/plugin ...`: returns what to show in the conversation.
    pub(in crate::tui) fn plugin_command(&mut self, arguments: &str) -> Result<String> {
        let (word, rest) = match arguments.split_once(char::is_whitespace) {
            Some((word, rest)) => (word, rest.trim()),
            None => (arguments, ""),
        };
        match word {
            "" | "list" if rest.is_empty() => Ok(self.plugin_list()),
            "install" => Ok(self.start_plugin_install(rest)),
            "remove" if !rest.is_empty() => {
                Ok(match plugins::remove(&self.extensions.dirs, rest) {
                    Ok(()) => {
                        self.forget_plugin(rest);
                        write_settings(&self.settings)?;
                        format!("Removed the plugin {rest}.")
                    }
                    Err(error) => format!("Could not remove the plugin: {error:#}"),
                })
            }
            _ => Ok(USAGE.to_owned()),
        }
    }

    fn plugin_list(&self) -> String {
        let (installed, warnings) = plugins::installed(&self.extensions.dirs, &self.settings);
        let mut text = if installed.is_empty() {
            "No plugins are installed. Install one with /plugin install <git-url or folder>."
                .to_owned()
        } else {
            "Plugins (switch them on or off in Settings > Plugins):".to_owned()
        };
        for plugin in &installed {
            let manifest = &plugin.manifest;
            text.push_str(&format!(
                "\n  {} {} ({}): {} · {} skill folder(s), {} command(s), {} mod(s)",
                manifest.name,
                manifest.version,
                if plugin.enabled { "on" } else { "off" },
                manifest.description,
                manifest.skills.len(),
                manifest.commands.len(),
                manifest.mods.len()
            ));
        }
        for warning in warnings {
            text.push_str(&format!("\n  Skipped {warning}"));
        }
        text
    }

    fn start_plugin_install(&mut self, source: &str) -> String {
        if source.is_empty() {
            return "Say where the plugin is: /plugin install <Git address or a folder>."
                .to_owned();
        }
        if self.extensions.install.is_some() {
            return "Another plugin install is still waiting; answer it first.".to_owned();
        }
        let parsed = match plugins::parse_source(source) {
            Ok(parsed) => parsed,
            Err(error) => return format!("Could not install the plugin: {error:#}"),
        };
        let dirs = self.extensions.dirs.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ =
                sender.send(plugins::stage(&dirs, &parsed).map_err(|error| format!("{error:#}")));
        });
        self.extensions.install = Some(Install::Fetching(receiver));
        self.notice = "Fetching the plugin…".to_owned();
        format!("Fetching {source}. You will be asked before anything is installed.")
    }

    /// Picks up a finished fetch.
    pub(in crate::tui) fn poll_plugin_install(&mut self) {
        let Some(Install::Fetching(receiver)) = self.extensions.install.as_ref() else {
            return;
        };
        let fetched = match receiver.try_recv() {
            Ok(fetched) => fetched,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the fetch stopped unexpectedly".to_owned())
            }
        };
        match fetched {
            Ok(staged) => {
                let lines = staged.review(&crate::tui::extensions::built_in);
                self.extensions.install = Some(Install::Review {
                    staged: Box::new(staged),
                    lines,
                    scroll: 0,
                });
                self.notice =
                    "Read what the plugin adds, then press y to install it or n to cancel."
                        .to_owned();
            }
            Err(error) => {
                self.extensions.install = None;
                self.notice = format!("Could not install the plugin: {error}");
                self.finish_command(self.notice.clone());
            }
        }
    }

    pub(in crate::tui) fn handle_plugin_review_key(&mut self, key: event::KeyEvent) -> Result<()> {
        let Some(Install::Review { scroll, .. }) = self.extensions.install.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                if let Some(Install::Review { staged, .. }) = self.extensions.install.take() {
                    staged.discard();
                }
                self.notice = "Nothing was installed.".to_owned();
            }
            KeyCode::Char('y' | 'Y') => {
                if let Some(Install::Review { staged, .. }) = self.extensions.install.take() {
                    self.install_reviewed(*staged)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Keeps a plugin the user said yes to, and approves the mods they were shown.
    fn install_reviewed(&mut self, staged: Staged) -> Result<()> {
        let name = staged.manifest.name.clone();
        let mods = staged.mods();
        let summary = format!(
            "Installed {name}: {} skill(s), {} command(s), {} mod(s).",
            staged.skills.len(),
            staged.manifest.commands.len(),
            mods.len()
        );
        if let Err(error) = staged.install() {
            self.notice = format!("Could not install the plugin: {error:#}");
            return Ok(());
        }
        for info in &mods {
            crate::extensions::mods::approve(&mut self.settings, info);
        }
        self.settings
            .disabled_plugins
            .retain(|plugin| *plugin != name);
        write_settings(&self.settings)?;
        self.forget_extension_items();
        self.sync_mods();
        self.notice = summary.clone();
        self.finish_command(summary);
        Ok(())
    }

    /// Forgets a removed plugin's switches and mod approvals and stops its mods.
    pub(in crate::tui) fn forget_plugin(&mut self, name: &str) {
        let prefix = format!("{name}/");
        self.settings
            .disabled_plugins
            .retain(|plugin| plugin != name);
        self.settings
            .approved_mods
            .retain(|id, _| !id.starts_with(&prefix));
        self.settings
            .disabled_mods
            .retain(|id| !id.starts_with(&prefix));
        for id in self.extensions.mods.running_ids() {
            if id.starts_with(&prefix) {
                self.extensions.mods.stop(&id);
            }
        }
        self.forget_extension_items();
    }
}

/// The review of a plugin before it is installed.
pub(in crate::tui) fn draw_plugin_review(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let Some(Install::Review { lines, scroll, .. }) = app.extensions.install.as_ref() else {
        return;
    };
    let popup = centered_rect(80, 80, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Install this plugin? ")
        .title_bottom(Line::from(" y install · n cancel · ↑/↓ scroll ").right_aligned())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(255, 197, 92)))
        .style(Style::default().bg(crate::tui::theme::dialog()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut text = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let style = if index == 0 {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if line.ends_with(':') {
                Style::default().fg(crate::tui::theme::accent())
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(Span::styled(line.clone(), style))
        })
        .collect::<Vec<_>>();
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        "Nothing from this plugin runs unless you press y. Its mods start right away after that and can be switched off in Settings > Plugins.",
        Style::default().fg(Color::Rgb(255, 197, 92)),
    )));
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((*scroll, 0)),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use crate::extensions::Dirs;
    use crate::tui::state::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("coolcode-install-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("folder");
        path.canonicalize().expect("real")
    }

    /// A plugin with a skill, a command and a mod.
    pub(in crate::tui) fn plugin_source() -> PathBuf {
        let folder = temp("source");
        std::fs::write(
            folder.join("plugin.toml"),
            r#"name = "reviewer"
version = "1.2.0"
description = "Code review helpers"
skills = ["skills"]

[[commands]]
name = "review"
description = "Review the staged changes"
prompt = "Review the staged changes. Focus on $ARGUMENTS."

[[mods]]
name = "status"
command = "python3"
args = ["mods/status.py", "--every", "5 s"]
events = ["turn_finished"]
"#,
        )
        .unwrap();
        let skill = folder.join("skills/checklist");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: checklist\ndescription: A review checklist\n---\nCheck.\n",
        )
        .unwrap();
        folder
    }

    fn app_in(home: &Path) -> App {
        let mut app = App::new(crate::Settings::default());
        app.trust_prompt = false;
        app.workspace_trusted = true;
        app.extensions.dirs = Dirs::under(home);
        app
    }

    fn run(app: &mut App, command: &str) -> String {
        app.set_input(command);
        app.submit().expect("submit");
        app.transcript
            .last()
            .map(|entry| entry.text.clone())
            .unwrap_or_default()
    }

    fn press(app: &mut App, code: KeyCode) {
        crate::tui::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    }

    fn wait_for_review(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !app.plugin_review_open() {
            assert!(
                Instant::now() < deadline,
                "no review; notice: {}",
                app.notice
            );
            app.poll_response();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn screen(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");
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
    fn plugin_without_a_known_word_explains_itself_and_lists_nothing_yet() {
        let home = temp("home");
        let mut app = app_in(&home);
        assert!(run(&mut app, "/plugin").contains("No plugins are installed"));
        assert!(run(&mut app, "/plugin list").contains("No plugins are installed"));
        assert!(run(&mut app, "/plugin frobnicate").contains("Usage: /plugin install"));
        assert!(run(&mut app, "/plugin install").contains("Git address or a folder"));
        assert!(run(&mut app, "/plugin remove ../x").contains("not a plugin name"));
    }

    #[test]
    fn an_install_shows_everything_first_and_saying_no_installs_nothing() {
        let home = temp("home");
        let mut app = app_in(&home);
        let source = plugin_source();
        let said = run(&mut app, &format!("/plugin install {}", source.display()));
        assert!(said.contains("asked before"), "{said}");
        wait_for_review(&mut app);
        let shown = screen(&app);
        for expected in [
            "Install this plugin?",
            "reviewer 1.2.0",
            "checklist: A review checklist",
            "/review: Review the staged changes",
            "python3 mods/status.py --every \"5 s\"",
            "turn_finished",
        ] {
            assert!(shown.contains(expected), "{expected}:\n{shown}");
        }
        let target = home.join(".coolcode/plugins/reviewer");
        assert!(!target.exists(), "nothing is installed before the answer");
        press(&mut app, KeyCode::Char('x'));
        assert!(
            app.input.is_empty(),
            "keys go to the review, not the prompt"
        );
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.plugin_review_open());
        assert!(!target.exists());
        assert!(app.settings.approved_mods.is_empty());
        assert!(
            app.notice.contains("Nothing was installed"),
            "{}",
            app.notice
        );
    }

    #[test]
    fn saying_yes_installs_it_with_its_skills_commands_and_approved_mods() {
        let home = temp("home");
        let mut app = app_in(&home);
        run(
            &mut app,
            &format!("/plugin install {}", plugin_source().display()),
        );
        wait_for_review(&mut app);
        press(&mut app, KeyCode::Char('y'));
        let target = home.join(".coolcode/plugins/reviewer");
        assert!(target.join("plugin.toml").is_file());
        assert!(app.notice.contains("Installed reviewer"), "{}", app.notice);
        let approved = app
            .settings
            .approved_mods
            .get("reviewer/status")
            .expect("approved");
        let (mods, _) = crate::extensions::mods::discover(&app.extensions.dirs, &app.settings);
        assert_eq!(*approved, crate::extensions::mods::manifest_hash(&mods[0]));
        assert!(run(&mut app, "/plugin list").contains("reviewer 1.2.0"));
        let catalog = app.slash_catalog();
        assert!(catalog.iter().any(|item| item.name == "review"));
        assert!(catalog.iter().any(|item| item.name == "checklist"));
        app.set_input("/review the tests");
        app.submit().expect("command");
        let sent = app.messages.last().expect("sent");
        assert_eq!(
            sent.display,
            "Review the staged changes. Focus on the tests."
        );
    }

    #[test]
    fn removing_a_plugin_deletes_it_and_forgets_its_approvals() {
        let home = temp("home");
        let mut app = app_in(&home);
        run(
            &mut app,
            &format!("/plugin install {}", plugin_source().display()),
        );
        wait_for_review(&mut app);
        press(&mut app, KeyCode::Char('y'));
        app.settings.disabled_plugins.push("reviewer".to_owned());
        let said = run(&mut app, "/plugin remove reviewer");
        assert!(said.contains("Removed"), "{said}");
        assert!(!home.join(".coolcode/plugins/reviewer").exists());
        assert!(app.settings.approved_mods.is_empty());
        assert!(app.settings.disabled_plugins.is_empty());
        assert!(!app.slash_catalog().iter().any(|item| item.name == "review"));
    }

    #[test]
    fn a_failed_fetch_is_reported_and_leaves_nothing_behind() {
        let home = temp("home");
        let mut app = app_in(&home);
        let empty = temp("empty");
        run(&mut app, &format!("/plugin install {}", empty.display()));
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.extensions.install.is_some() {
            assert!(Instant::now() < deadline);
            app.poll_response();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.notice.contains("Could not install"), "{}", app.notice);
        assert!(app.notice.contains("plugin.toml"), "{}", app.notice);
    }
}
