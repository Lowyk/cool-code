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

#[cfg(test)]
mod tests {
    use super::{auto_approve_command, auto_approve_edit};

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
