pub(crate) const MODES: [(&str, &str); 6] = [
    ("Auto", "auto"),
    ("Accept Edits", "accept-edits"),
    ("Accept Minimal", "accept-minimal"),
    ("Accept Everything", "accept-everything"),
    ("Manual", "manual"),
    ("Plan", "plan"),
];

/// Where `mode` sits in [`MODES`]; an unknown mode lands on Plan, the safest one.
pub(crate) fn mode_index(mode: &str) -> usize {
    MODES
        .iter()
        .position(|(_, value)| *value == mode)
        .unwrap_or(MODES.len() - 1)
}

/// Whether a mode approves edits and new files without asking. Auto does not: its guard models
/// decide each one.
pub(crate) fn auto_approve_file_change(permission_mode: &str) -> bool {
    matches!(
        permission_mode,
        "accept-everything" | "accept-edits" | "accept-minimal"
    )
}

/// Whether a mode lets the model call a tool of an MCP server without asking. Only Accept
/// Everything does: the program behind a tool is not Cool Code's own, and its description of
/// what a tool does is its own claim. Auto mode leaves each call to the guard models.
pub(crate) fn auto_approve_mcp(permission_mode: &str) -> bool {
    permission_mode == "accept-everything"
}

pub(crate) fn auto_approve_command(permission_mode: &str, command: &str) -> bool {
    if permission_mode == "accept-everything" {
        return true;
    }
    if permission_mode != "accept-minimal" {
        return false;
    }
    let normalized = command.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "cargo fmt --check"
            | "cargo check"
            | "cargo test"
            | "npm test"
            | "npm run build"
            | "pytest"
    )
}

/// The modes Shift+Tab goes through, in order. Accept Everything is left out on purpose: one
/// stray key must never switch off every question, so it is only reached by choosing it.
const CYCLE: [&str; 5] = ["auto", "accept-edits", "accept-minimal", "manual", "plan"];

/// The mode after `current` for Shift+Tab. Auto is skipped while no guard model can judge
/// actions. From a mode outside the cycle (Accept Everything) it goes to Manual, the next one in
/// the list of modes.
pub(crate) fn next_mode(current: &str, auto_ready: bool) -> &'static str {
    let start = match CYCLE.iter().position(|mode| *mode == current) {
        Some(at) => at + 1,
        None => CYCLE.iter().position(|mode| *mode == "manual").unwrap_or(0),
    };
    (0..CYCLE.len())
        .map(|step| CYCLE[(start + step) % CYCLE.len()])
        .find(|mode| *mode != "auto" || auto_ready)
        .unwrap_or("manual")
}

pub(crate) fn mode_label(mode: &str) -> &'static str {
    MODES[mode_index(mode)].0
}

#[cfg(test)]
mod tests {
    use super::{auto_approve_command, auto_approve_file_change};

    const ALLOWLISTED_COMMANDS: [&str; 6] = [
        "cargo fmt --check",
        "cargo check",
        "cargo test",
        "npm test",
        "npm run build",
        "pytest",
    ];

    #[test]
    fn modes_that_do_not_approve_file_changes_leave_them_to_the_user_or_guard() {
        for mode in ["", "AUTO", "Plan", "yolo", "auto", "manual", "plan"] {
            assert!(!auto_approve_file_change(mode), "{mode:?}");
            assert!(!auto_approve_command(mode, "cargo test"), "{mode:?}");
        }
    }

    #[test]
    fn accept_modes_approve_file_changes() {
        for mode in ["accept-edits", "accept-minimal", "accept-everything"] {
            assert!(auto_approve_file_change(mode), "{mode}");
        }
    }

    #[test]
    fn auto_mode_leaves_every_command_to_the_guard() {
        for command in ALLOWLISTED_COMMANDS {
            assert!(!auto_approve_command("auto", command), "{command}");
        }
    }

    #[test]
    fn manual_mode_asks_before_every_command() {
        for command in ALLOWLISTED_COMMANDS {
            assert!(!auto_approve_command("manual", command), "{command}");
        }
    }

    #[test]
    fn shift_tab_goes_round_the_modes_without_ever_entering_accept_everything() {
        let mut mode = "auto";
        let mut seen = Vec::new();
        for _ in 0..5 {
            mode = super::next_mode(mode, true);
            seen.push(mode);
        }
        assert_eq!(
            seen,
            ["accept-edits", "accept-minimal", "manual", "plan", "auto"]
        );
        // Auto is skipped while it cannot be used.
        assert_eq!(super::next_mode("plan", false), "accept-edits");
        assert_eq!(super::next_mode("manual", false), "plan");
        // Leaving Accept Everything goes to Manual, and nothing leads into it.
        assert_eq!(super::next_mode("accept-everything", true), "manual");
        for mode in [
            "",
            "auto",
            "accept-edits",
            "accept-minimal",
            "manual",
            "plan",
            "nonsense",
        ] {
            for ready in [true, false] {
                assert_ne!(super::next_mode(mode, ready), "accept-everything", "{mode}");
            }
        }
    }

    #[test]
    fn manual_is_a_listed_mode_with_a_label() {
        assert!(super::MODES.iter().any(|(_, value)| *value == "manual"));
        assert_eq!(super::mode_label("manual"), "Manual");
    }

    #[test]
    fn every_mode_has_a_position_and_unknown_ones_land_on_plan() {
        for (index, (_, mode)) in super::MODES.iter().enumerate() {
            assert_eq!(super::mode_index(mode), index, "{mode}");
        }
        assert_eq!(super::MODES[super::mode_index("nonsense")].1, "plan");
    }

    #[test]
    fn command_allowlist_is_exact_after_trim_and_lowercase() {
        for command in ALLOWLISTED_COMMANDS {
            assert!(auto_approve_command("accept-minimal", command), "{command}");
        }
        assert!(auto_approve_command("accept-minimal", " CARGO TEST "));
        for command in [
            "cargo test --release",
            "cargo test && echo x",
            "cargo  test",
        ] {
            assert!(
                !auto_approve_command("accept-minimal", command),
                "{command}"
            );
        }
        for command in ALLOWLISTED_COMMANDS {
            assert!(!auto_approve_command("accept-edits", command), "{command}");
        }
    }

    #[test]
    fn accept_everything_approves_any_command() {
        assert!(auto_approve_command(
            "accept-everything",
            "Remove-Item -Recurse ."
        ));
        assert!(!auto_approve_command(
            "accept-minimal",
            "cargo test; del important.txt"
        ));
    }
}
