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
