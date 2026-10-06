pub(crate) const MODES: [(&str, &str); 5] = [
    ("Auto", "auto"),
    ("Accept Edits", "accept-edits"),
    ("Accept Minimal", "accept-minimal"),
    ("Accept Everything", "accept-everything"),
    ("Plan", "plan"),
];

pub(crate) fn auto_approve_create(
    permission_mode: &str,
    proposal: &crate::tools::CreateProposal,
) -> bool {
    match permission_mode {
        "accept-everything" | "accept-edits" | "accept-minimal" => true,
        "auto" => {
            let path = proposal
                .relative_path
                .replace('\\', "/")
                .to_ascii_lowercase();
            proposal.is_small()
                && !path.split('/').any(|part| {
                    part.starts_with(".env")
                        || part.contains("secret")
                        || part.contains("credential")
                })
        }
        _ => false,
    }
}

pub(crate) fn auto_approve_edit(
    permission_mode: &str,
    proposal: &crate::tools::EditProposal,
) -> bool {
    match permission_mode {
        "accept-everything" | "accept-edits" => true,
        "accept-minimal" => true,
        "auto" => {
            let path = proposal
                .relative_path
                .replace('\\', "/")
                .to_ascii_lowercase();
            proposal.is_small()
                && !path.split('/').any(|part| {
                    part.starts_with(".env")
                        || part.contains("secret")
                        || part.contains("credential")
                })
        }
        _ => false,
    }
}

pub(crate) fn auto_approve_command(permission_mode: &str, command: &str) -> bool {
    if permission_mode == "accept-everything" {
        return true;
    }
    if !matches!(permission_mode, "auto" | "accept-minimal") {
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
    MODES
        .iter()
        .find(|(_, value)| *value == mode)
        .map(|(label, _)| *label)
        .unwrap_or("Plan")
}

#[cfg(test)]
mod tests {
    use super::{auto_approve_command, auto_approve_create, auto_approve_edit};
    use crate::tools::{CreateProposal, EditProposal};

    const ALL_APPROVING_MODES: [&str; 3] = ["accept-edits", "accept-minimal", "accept-everything"];
    const ALLOWLISTED_COMMANDS: [&str; 6] = [
        "cargo fmt --check",
        "cargo check",
        "cargo test",
        "npm test",
        "npm run build",
        "pytest",
    ];

    fn edit(path: &str, before: &str, after: &str) -> EditProposal {
        EditProposal {
            relative_path: path.to_owned(),
            original: before.to_owned(),
            updated: after.to_owned(),
            change_summary: "Replace lines 1-1".to_owned(),
            before: before.to_owned(),
            after: after.to_owned(),
        }
    }

    fn create(path: &str, content: &str) -> CreateProposal {
        CreateProposal {
            relative_path: path.to_owned(),
            content: content.to_owned(),
        }
    }

    #[test]
    fn unknown_modes_approve_nothing() {
        for mode in ["", "AUTO", "Plan", "yolo"] {
            assert!(
                !auto_approve_edit(mode, &edit("src/main.rs", "a", "b")),
                "{mode:?}"
            );
            assert!(
                !auto_approve_create(mode, &create("src/new.rs", "x")),
                "{mode:?}"
            );
            assert!(!auto_approve_command(mode, "cargo test"), "{mode:?}");
        }
    }

    #[test]
    fn plan_mode_approves_nothing() {
        assert!(!auto_approve_edit("plan", &edit("src/main.rs", "a", "b")));
        assert!(!auto_approve_create("plan", &create("src/new.rs", "x")));
        for command in ALLOWLISTED_COMMANDS {
            assert!(!auto_approve_command("plan", command), "{command}");
        }
    }

    #[test]
    fn accept_modes_approve_edits_and_creates_regardless_of_path_or_size() {
        let large = "a".repeat(10_000);
        for mode in ALL_APPROVING_MODES {
            assert!(
                auto_approve_edit(mode, &edit(".env", &large, &large)),
                "{mode}"
            );
            assert!(auto_approve_create(mode, &create(".env", &large)), "{mode}");
        }
    }

    #[test]
    fn auto_mode_requires_approval_for_sensitive_paths() {
        for path in [
            ".env",
            ".ENV.local",
            "config\\Secrets.json",
            "src/my_credentials.rs",
        ] {
            assert!(!auto_approve_edit("auto", &edit(path, "a", "b")), "{path}");
            assert!(!auto_approve_create("auto", &create(path, "x")), "{path}");
        }
        assert!(auto_approve_edit("auto", &edit("src/main.rs", "a", "b")));
        assert!(auto_approve_create("auto", &create("src/main.rs", "x")));
    }

    #[test]
    fn auto_mode_size_boundary_is_2048_bytes() {
        let at_limit = "a".repeat(2048);
        let over_limit = "a".repeat(2049);
        assert!(auto_approve_create(
            "auto",
            &create("src/new.rs", &at_limit)
        ));
        assert!(!auto_approve_create(
            "auto",
            &create("src/new.rs", &over_limit)
        ));
        assert!(auto_approve_edit(
            "auto",
            &edit("src/a.rs", &at_limit, &at_limit)
        ));
        assert!(!auto_approve_edit(
            "auto",
            &edit("src/a.rs", &at_limit, &over_limit)
        ));
        assert!(!auto_approve_edit(
            "auto",
            &edit("src/a.rs", &over_limit, &at_limit)
        ));
    }

    #[test]
    fn command_allowlist_is_exact_after_trim_and_lowercase() {
        for mode in ["auto", "accept-minimal"] {
            for command in ALLOWLISTED_COMMANDS {
                assert!(auto_approve_command(mode, command), "{mode}: {command}");
            }
            assert!(auto_approve_command(mode, " CARGO TEST "), "{mode}");
            for command in [
                "cargo test --release",
                "cargo test && echo x",
                "cargo  test",
            ] {
                assert!(!auto_approve_command(mode, command), "{mode}: {command}");
            }
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
    }

    #[test]
    fn permission_modes_have_deterministic_edit_and_command_rules() {
        let proposal = crate::tools::EditProposal {
            relative_path: "src/main.rs".to_owned(),
            original: "old".to_owned(),
            updated: "new".to_owned(),
            change_summary: "Replace lines 1-1".to_owned(),
            before: "old".to_owned(),
            after: "new".to_owned(),
        };
        assert!(!auto_approve_edit("plan", &proposal));
        assert!(auto_approve_edit("accept-edits", &proposal));
        assert!(auto_approve_edit("accept-minimal", &proposal));
        assert!(auto_approve_edit("auto", &proposal));
        assert!(auto_approve_edit("accept-everything", &proposal));
        assert!(!auto_approve_command("plan", "cargo test"));
        assert!(!auto_approve_command("accept-edits", "cargo test"));
        assert!(auto_approve_command("accept-minimal", "cargo test"));
        assert!(!auto_approve_command(
            "accept-minimal",
            "cargo test; del important.txt"
        ));
        assert!(auto_approve_command("auto", "npm test"));
        assert!(auto_approve_command(
            "accept-everything",
            "Remove-Item -Recurse ."
        ));
    }
}
