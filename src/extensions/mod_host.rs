//! Running mods: each one is a child process started off the interface thread, with a thread
//! that writes its events and one that reads its messages. Events are dropped rather than
//! waited for, output is capped in size and rate, and every mod is killed when the host stops.

use crate::extensions::mods::{
    Event, MAX_STATUS_CHARS, MAX_TOAST_CHARS, ModInfo, ModMessage, parse_line, sanitize,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Events waiting for a mod that is slow to read them; more are dropped.
const QUEUED_EVENTS: usize = 64;
/// Longest line a mod may print.
const MAX_LINE_BYTES: usize = 4 * 1024;
/// Most output a mod may print in all before it is stopped.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Most messages a mod may send in one second; the rest are dropped.
const MAX_MESSAGES_PER_SECOND: usize = 20;
/// The only variables from the user's environment a mod gets (compared without case, for
/// Windows). Everything else, API keys included, stays behind.
const PASSED_VARIABLES: [&str; 18] = [
    "PATH",
    "PATHEXT",
    "HOME",
    "USER",
    "USERNAME",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "WINDIR",
    "USERPROFILE",
    "COMSPEC",
];

/// The program to start: an absolute path as written, a relative path inside the mod's folder,
/// or a bare name looked up on PATH.
fn program(info: &ModInfo) -> Result<PathBuf, String> {
    let command = Path::new(&info.manifest.command);
    if command.is_absolute() {
        return Ok(command.to_path_buf());
    }
    if !info.manifest.command.contains(['/', '\\']) {
        return Ok(command.to_path_buf());
    }
    let folder = info
        .dir
        .canonicalize()
        .map_err(|error| format!("its folder cannot be read ({error})"))?;
    let real = folder
        .join(command)
        .canonicalize()
        .map_err(|error| format!("{} cannot be found ({error})", info.manifest.command))?;
    if !real.starts_with(&folder) {
        return Err(format!(
            "{} leads outside the mod's folder",
            info.manifest.command
        ));
    }
    Ok(real)
}

/// What a mod's threads report to the interface.
#[derive(Debug)]
enum Update {
    Message { id: String, message: ModMessage },
    Problem { id: String, text: String },
    Exited { id: String, code: Option<i32> },
}

struct Running {
    info: ModInfo,
    input: SyncSender<String>,
    child: Arc<Mutex<Option<Child>>>,
    stopping: Arc<AtomicBool>,
    status: String,
    /// Already told the user that events are being dropped.
    dropped_events: bool,
}

/// The running mods.
pub(crate) struct ModHost {
    running: Vec<Running>,
    sender: Sender<Update>,
    updates: Receiver<Update>,
    notices: Vec<String>,
}

impl Default for ModHost {
    fn default() -> Self {
        let (sender, updates) = std::sync::mpsc::channel();
        ModHost {
            running: Vec::new(),
            sender,
            updates,
            notices: Vec::new(),
        }
    }
}

impl ModHost {
    /// Starts a mod (without waiting for it) and gives it `first` if it asked for that event.
    pub(crate) fn start(&mut self, info: ModInfo, first: Option<&Event>) {
        self.stop(&info.id);
        let (input, queued) = std::sync::mpsc::sync_channel::<String>(QUEUED_EVENTS);
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_info = info.clone();
        let thread_child = child.clone();
        let thread_stopping = stopping.clone();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            run(thread_info, queued, thread_child, thread_stopping, sender);
        });
        let mut running = Running {
            info,
            input,
            child,
            stopping,
            status: String::new(),
            dropped_events: false,
        };
        if let Some(event) = first {
            deliver(&mut running, event, &mut self.notices);
        }
        self.running.push(running);
    }

    /// What a running mod was started with.
    pub(crate) fn running_info(&self, id: &str) -> Option<&ModInfo> {
        self.running
            .iter()
            .find(|running| running.info.id == id)
            .map(|running| &running.info)
    }

    /// The ids of the mods that are running.
    pub(crate) fn running_ids(&self) -> Vec<String> {
        self.running
            .iter()
            .map(|mod_| mod_.info.id.clone())
            .collect()
    }

    /// Sends `event` to every mod that asked for it, never waiting for one.
    pub(crate) fn send(&mut self, event: &Event) {
        for running in &mut self.running {
            deliver(running, event, &mut self.notices);
        }
    }

    /// Applies what the mods sent since the last call; returns the notices to show.
    pub(crate) fn poll(&mut self) -> Vec<String> {
        while let Ok(update) = self.updates.try_recv() {
            match update {
                Update::Message { id, message } => {
                    let Some(running) = self.running.iter_mut().find(|r| r.info.id == id) else {
                        continue;
                    };
                    match message {
                        ModMessage::Status(text) => {
                            running.status = sanitize(&text, MAX_STATUS_CHARS);
                        }
                        ModMessage::Toast(text) => {
                            let text = sanitize(&text, MAX_TOAST_CHARS);
                            if !text.is_empty() {
                                self.notices
                                    .push(format!("{}: {text}", running.info.manifest.name));
                            }
                        }
                    }
                }
                Update::Problem { id, text } => {
                    if self.running.iter().any(|r| r.info.id == id) {
                        self.notices.push(format!("Mod {id} {text}"));
                    }
                }
                Update::Exited { id, code } => {
                    if let Some(index) = self.running.iter().position(|r| r.info.id == id) {
                        self.running.remove(index);
                        self.notices.push(match code {
                            Some(code) => format!("Mod {id} stopped (exit status {code})."),
                            None => format!("Mod {id} stopped."),
                        });
                    }
                }
            }
        }
        std::mem::take(&mut self.notices)
    }

    /// The status texts to show, one per mod that set one.
    pub(crate) fn statuses(&self) -> Vec<String> {
        self.running
            .iter()
            .filter(|running| !running.status.is_empty())
            .map(|running| running.status.clone())
            .collect()
    }

    /// Stops one mod.
    pub(crate) fn stop(&mut self, id: &str) {
        if let Some(index) = self.running.iter().position(|r| r.info.id == id) {
            let running = self.running.remove(index);
            running.stopping.store(true, Ordering::Relaxed);
            if let Some(child) = lock(&running.child).as_mut() {
                let _ = child.kill();
            }
        }
    }

    /// Stops every mod.
    pub(crate) fn stop_all(&mut self) {
        for id in self.running_ids() {
            self.stop(&id);
        }
    }
}

fn lock(child: &Mutex<Option<Child>>) -> std::sync::MutexGuard<'_, Option<Child>> {
    child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Queues `event` for one mod if it asked for it; a full queue drops it.
fn deliver(running: &mut Running, event: &Event, notices: &mut Vec<String>) {
    if !running
        .info
        .manifest
        .events
        .iter()
        .any(|name| name == event.name())
    {
        return;
    }
    if let Err(TrySendError::Full(_)) = running.input.try_send(event.to_line())
        && !running.dropped_events
    {
        running.dropped_events = true;
        notices.push(format!(
            "Mod {} is not reading its events; some were dropped.",
            running.info.id
        ));
    }
}

/// A mod's life, on its own thread: start it, feed it events, read its messages, and report
/// how it ended.
fn run(
    info: ModInfo,
    queued: Receiver<String>,
    child: Arc<Mutex<Option<Child>>>,
    stopping: Arc<AtomicBool>,
    sender: Sender<Update>,
) {
    let id = info.id.clone();
    let problem = |text: String| {
        let _ = sender.send(Update::Problem {
            id: id.clone(),
            text,
        });
    };
    let started = program(&info).and_then(|program| {
        Command::new(program)
            .args(&info.manifest.args)
            .current_dir(&info.dir)
            .env_clear()
            .envs(std::env::vars().filter(|(name, _)| {
                PASSED_VARIABLES
                    .iter()
                    .any(|passed| passed.eq_ignore_ascii_case(name))
            }))
            .env("COOLCODE_MOD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())
    });
    let mut process = match started {
        Ok(process) => process,
        Err(reason) => {
            problem(format!("could not start: {reason}"));
            let _ = sender.send(Update::Exited {
                id: id.clone(),
                code: None,
            });
            return;
        }
    };
    let stdin = process.stdin.take();
    let stdout = process.stdout.take();
    *lock(&child) = Some(process);
    if stopping.load(Ordering::Relaxed)
        && let Some(process) = lock(&child).as_mut()
    {
        let _ = process.kill();
    }
    if let Some(mut stdin) = stdin {
        std::thread::spawn(move || {
            for line in queued {
                if writeln!(stdin, "{line}")
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    if let Some(stdout) = stdout {
        read_messages(stdout, &id, &sender, &child);
    }
    // The output closed: wait briefly for the exit status, then make sure it is gone.
    let deadline = Instant::now() + Duration::from_secs(2);
    let code = loop {
        let mut slot = lock(&child);
        let Some(process) = slot.as_mut() else {
            break None;
        };
        match process.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => {}
            _ => {
                let _ = process.kill();
                let _ = process.wait();
                break None;
            }
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = sender.send(Update::Exited { id, code });
}

/// Reads a mod's messages until its output closes, within the size and rate limits.
fn read_messages(
    stdout: impl Read,
    id: &str,
    sender: &Sender<Update>,
    child: &Mutex<Option<Child>>,
) {
    let problem = |text: &str| {
        let _ = sender.send(Update::Problem {
            id: id.to_owned(),
            text: text.to_owned(),
        });
    };
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut total = 0usize;
    let mut second = (Instant::now(), 0usize);
    let (mut told_garbage, mut told_rate) = (false, false);
    loop {
        line.clear();
        let Ok(read) = (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
        else {
            break;
        };
        if read == 0 {
            break;
        }
        total += read;
        let mut too_long = false;
        // Skip the rest of a line that is too long.
        while !line.ends_with(b"\n") && line.len() > MAX_LINE_BYTES && total <= MAX_OUTPUT_BYTES {
            too_long = true;
            let mut rest = Vec::new();
            match (&mut reader)
                .take(MAX_LINE_BYTES as u64)
                .read_until(b'\n', &mut rest)
            {
                Ok(0) | Err(_) => break,
                Ok(more) => {
                    total += more;
                    if rest.ends_with(b"\n") {
                        break;
                    }
                }
            }
        }
        if total > MAX_OUTPUT_BYTES {
            problem("printed more than 1 MiB and was stopped.");
            if let Some(process) = lock(child).as_mut() {
                let _ = process.kill();
            }
            break;
        }
        if second.0.elapsed() >= Duration::from_secs(1) {
            second = (Instant::now(), 0);
        }
        second.1 += 1;
        if second.1 > MAX_MESSAGES_PER_SECOND {
            if !told_rate {
                told_rate = true;
                problem("is sending too many messages; some were dropped.");
            }
            continue;
        }
        let parsed = if too_long {
            Err("the line is too long".to_owned())
        } else {
            parse_line(&String::from_utf8_lossy(&line))
        };
        match parsed {
            Ok(messages) => {
                for message in messages {
                    let _ = sender.send(Update::Message {
                        id: id.to_owned(),
                        message,
                    });
                }
            }
            Err(reason) => {
                if !told_garbage {
                    told_garbage = true;
                    problem(&format!(
                        "printed a line that is not a message ({reason}); such lines are ignored."
                    ));
                }
            }
        }
    }
}

impl Drop for ModHost {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::extensions::mods::ModManifest;
    use std::time::{Duration, Instant};

    /// Marks the test binary's own run as a mod (see [`mod_helper_process`]).
    const HELPER: &str = "coolcode-mod-helper";

    /// This test binary, run as a mod in one of several behaviours.
    pub(crate) fn helper(mode: &str, events: &[&str]) -> ModInfo {
        let exe = std::env::current_exe().expect("test binary");
        ModInfo {
            id: format!("helper-{mode}"),
            manifest: ModManifest {
                name: format!("helper-{mode}"),
                description: String::new(),
                command: exe.display().to_string(),
                args: [
                    "--exact",
                    "extensions::mod_host::tests::mod_helper_process",
                    "--nocapture",
                    "--test-threads=1",
                    HELPER,
                    mode,
                ]
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect(),
                events: events.iter().map(|event| (*event).to_owned()).collect(),
            },
            dir: std::env::temp_dir(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_relative_command_that_links_outside_the_mod_folder_is_refused() {
        let folder =
            std::env::temp_dir().join(format!("coolcode-mod-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(folder.join("bin")).unwrap();
        std::fs::write(folder.join("bin/inside.sh"), "").unwrap();
        std::os::unix::fs::symlink("/bin/sh", folder.join("bin/outside")).unwrap();
        let mut info = helper("echo", &[]);
        info.dir = folder.clone();
        info.manifest.command = "bin/inside.sh".to_owned();
        assert!(program(&info).is_ok());
        info.manifest.command = "bin/outside".to_owned();
        let error = program(&info).expect_err("a link out of the folder");
        assert!(error.contains("outside the mod's folder"), "{error}");
        std::fs::remove_dir_all(folder).ok();
    }

    /// Not a test of its own: when the host starts this binary as a mod, this is the mod.
    #[test]
    fn mod_helper_process() {
        use std::io::{BufRead, Write};
        let args = std::env::args().collect::<Vec<_>>();
        if !args.iter().any(|arg| arg == HELPER) {
            return;
        }
        let mode = args.last().cloned().unwrap_or_default();
        let mut out = std::io::stdout();
        // The test runner has printed "test <name> ... " without a line break; end that line.
        writeln!(out).unwrap();
        let wait_for_input = || for _ in std::io::stdin().lock().lines() {};
        match mode.as_str() {
            "echo" => {
                writeln!(out, r#"{{"status":"ready"}}"#).unwrap();
                for line in std::io::stdin().lock().lines() {
                    if line.unwrap().contains("turn_finished") {
                        writeln!(out, r#"{{"toast":"turn \u001b[31mdone"}}"#).unwrap();
                        writeln!(out, "not json").unwrap();
                        writeln!(out, r#"{{"status":"tick\nline"}}"#).unwrap();
                    }
                }
            }
            "crash" => {
                writeln!(out, r#"{{"status":"bye"}}"#).unwrap();
                out.flush().unwrap();
                std::process::exit(3);
            }
            "hang" => std::thread::sleep(Duration::from_secs(120)),
            "flood" => {
                for count in 0..3_000 {
                    writeln!(out, r#"{{"status":"n{count}"}}"#).unwrap();
                }
                writeln!(out, r#"{{"toast":"flood over"}}"#).unwrap();
                wait_for_input();
            }
            "env" => {
                let seen = std::env::var("CARGO_MANIFEST_DIR")
                    .map_or("none", |_| "leaked")
                    .to_owned();
                writeln!(out, r#"{{"status":"env {seen}"}}"#).unwrap();
                wait_for_input();
            }
            "huge" => {
                let chunk = "x".repeat(64 * 1024);
                for _ in 0..40 {
                    if out.write_all(chunk.as_bytes()).is_err() {
                        break;
                    }
                }
                wait_for_input();
            }
            _ => {}
        }
    }

    /// Polls until `done` holds for the notices so far, or fails after a while.
    fn wait_for(host: &mut ModHost, done: impl Fn(&ModHost, &[String]) -> bool) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut notices = Vec::new();
        loop {
            notices.extend(host.poll());
            if done(host, &notices) {
                return notices;
            }
            assert!(
                Instant::now() < deadline,
                "timed out; notices so far: {notices:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_mod_receives_events_and_shows_a_cleaned_status_and_notice() {
        let mut host = ModHost::default();
        host.start(helper("echo", &["turn_finished"]), None);
        let mut notices = wait_for(&mut host, |host, _| host.statuses() == ["ready"]);
        host.send(&Event::ToolStarted {
            tool: "read_file".to_owned(),
        });
        host.send(&Event::TurnFinished { outcome: "done" });
        notices.extend(wait_for(&mut host, |host, notices| {
            host.statuses() == ["tick line"] && notices.iter().any(|n| n.contains("turn [31mdone"))
        }));
        let garbage = notices
            .iter()
            .filter(|notice| notice.contains("not a message"))
            .count();
        assert_eq!(garbage, 1, "garbage is reported once: {notices:?}");
        assert!(notices.iter().all(|notice| !notice.contains('\u{1b}')));
        host.stop_all();
        assert!(host.running_ids().is_empty());
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            host.poll().iter().all(|notice| !notice.contains("stopped")),
            "a mod stopped on purpose is not reported as stopping by itself"
        );
    }

    #[test]
    fn a_mod_that_crashes_or_cannot_start_is_reported_and_the_rest_keep_going() {
        let mut host = ModHost::default();
        host.start(helper("crash", &[]), None);
        let mut missing = helper("missing", &[]);
        missing.manifest.command = "coolcode-no-such-program-anywhere".to_owned();
        host.start(missing, None);
        let notices = wait_for(&mut host, |_, notices| {
            notices
                .iter()
                .any(|n| n.contains("helper-crash") && n.contains("exit status 3"))
                && notices
                    .iter()
                    .any(|n| n.contains("helper-missing") && n.contains("could not start"))
        });
        assert!(host.running_ids().is_empty(), "{notices:?}");
        host.send(&Event::TurnFinished { outcome: "done" });
    }

    #[test]
    fn a_mod_that_does_not_read_never_blocks_and_is_killed_when_stopped() {
        let mut host = ModHost::default();
        host.start(helper("hang", &["tool_started"]), None);
        let started = Instant::now();
        for _ in 0..2_000 {
            host.send(&Event::ToolStarted {
                tool: "read_file".to_owned(),
            });
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let notices = wait_for(&mut host, |_, notices| {
            notices.iter().any(|n| n.contains("not reading"))
        });
        assert_eq!(
            notices.iter().filter(|n| n.contains("not reading")).count(),
            1,
            "{notices:?}"
        );
        let stopping = Instant::now();
        host.stop_all();
        assert!(stopping.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_flood_of_output_is_rate_limited() {
        let mut host = ModHost::default();
        host.start(helper("flood", &[]), None);
        let notices = wait_for(&mut host, |_, notices| {
            notices.iter().any(|n| n.contains("too many"))
        });
        assert!(
            notices.len() < 50,
            "the flood did not reach the screen: {}",
            notices.len()
        );
    }

    #[test]
    fn a_mod_does_not_see_the_users_environment() {
        assert!(
            std::env::var("CARGO_MANIFEST_DIR").is_ok(),
            "cargo gives tests this variable, so it is there to leak"
        );
        let mut host = ModHost::default();
        host.start(helper("env", &[]), None);
        wait_for(&mut host, |host, _| host.statuses() == ["env none"]);
    }

    #[test]
    fn a_mod_that_prints_too_much_is_stopped() {
        let mut host = ModHost::default();
        host.start(helper("huge", &[]), None);
        let notices = wait_for(&mut host, |host, notices| {
            host.running_ids().is_empty() && notices.iter().any(|n| n.contains("1 MiB"))
        });
        assert!(notices.iter().any(|n| n.contains("stopped")), "{notices:?}");
    }

    #[test]
    fn the_first_event_is_sent_only_if_the_mod_asked_for_it() {
        let mut host = ModHost::default();
        host.start(
            helper("echo", &["session_started"]),
            Some(&Event::TurnFinished { outcome: "done" }),
        );
        wait_for(&mut host, |host, _| host.statuses() == ["ready"]);
        std::thread::sleep(Duration::from_millis(300));
        host.poll();
        assert_eq!(host.statuses(), ["ready"], "turn_finished was not sent");
    }

    #[test]
    fn a_relative_command_must_stay_in_the_mods_folder() {
        let mut host = ModHost::default();
        let mut escaping = helper("escape", &[]);
        escaping.manifest.command = "../../bin/sh".to_owned();
        escaping.dir = std::env::temp_dir().join("coolcode-mod-escape");
        std::fs::create_dir_all(&escaping.dir).unwrap();
        host.start(escaping, None);
        wait_for(&mut host, |_, notices| {
            notices.iter().any(|n| n.contains("could not start"))
        });
    }
}
