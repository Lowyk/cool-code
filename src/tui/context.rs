use crate::provider;
use crate::tui::state::App;
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::path::{Path, PathBuf};

pub(super) fn read_cool_file() -> Result<Option<String>> {
    let root = std::env::current_dir()?
        .canonicalize()
        .context("resolving workspace root")?;
    let path = root.join("COOL.md");
    if !path.exists() {
        return Ok(None);
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    if !canonical.starts_with(&root) {
        bail!("COOL.md resolves outside the trusted workspace");
    }
    let metadata = std::fs::metadata(&canonical)
        .with_context(|| format!("reading {} metadata", canonical.display()))?;
    if metadata.len() > 64 * 1024 {
        bail!("COOL.md is larger than the 64 KiB context limit");
    }
    let contents = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    Ok(Some(contents))
}

pub(super) fn read_user_instructions() -> Result<Option<String>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let path = home.join(".coolcode").join("COOL.md");
    if !path.is_file() {
        return Ok(None);
    }
    let metadata =
        std::fs::metadata(&path).with_context(|| format!("reading {} metadata", path.display()))?;
    if metadata.len() > 64 * 1024 {
        bail!(
            "{} is larger than the 64 KiB user-instructions limit",
            path.display()
        );
    }
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("reading user instructions from {}", path.display()))?;
    Ok(Some(contents))
}

pub(super) fn workspace_is_trusted(root: &Path) -> bool {
    let marker = root.join(".coolcode").join("trusted");
    if !marker.is_file() {
        return false;
    }
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    marker
        .canonicalize()
        .ok()
        .is_some_and(|marker| marker.starts_with(root))
}

pub(super) fn build_user_message(
    prompt: &str,
    workspace_trusted: bool,
) -> Result<provider::ChatMessage> {
    if !workspace_trusted
        && prompt
            .split_whitespace()
            .any(|token| token.starts_with('@') && token.len() > 1)
    {
        bail!(
            "this workspace has not been trusted; @file references are disabled. Trust it from Settings → Privacy or restart and accept the workspace prompt"
        );
    }
    let root = std::env::current_dir()?
        .canonicalize()
        .context("resolving workspace root")?;
    let mut text = prompt.to_owned();
    let mut display = prompt.to_owned();
    let mut images = Vec::new();
    let mut attached = std::collections::HashSet::new();

    for token in prompt.split_whitespace() {
        let Some(raw_path) = token
            .strip_prefix('@')
            .map(|path| path.trim_end_matches([',', ';', ':', '!', '?', ')', ']', '}']))
        else {
            continue;
        };
        if raw_path.is_empty() || raw_path.contains('@') {
            continue;
        }
        let relative = PathBuf::from(raw_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("file references must stay inside the current workspace: @{raw_path}");
        }
        let joined = root.join(&relative);
        if !joined.exists() {
            if raw_path.contains('/') || raw_path.contains('\\') {
                bail!("referenced file does not exist: @{raw_path}");
            }
            continue;
        }
        let canonical = joined
            .canonicalize()
            .with_context(|| format!("resolving referenced file @{raw_path}"))?;
        if !canonical.starts_with(&root) {
            bail!("file references must stay inside the current workspace: @{raw_path}");
        }
        let relative_display = canonical
            .strip_prefix(&root)
            .unwrap_or(&canonical)
            .display()
            .to_string();
        if !attached.insert(relative_display.clone()) {
            continue;
        }
        let metadata = std::fs::metadata(&canonical)
            .with_context(|| format!("reading @{relative_display} metadata"))?;
        let extension = canonical
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let image_mime = match extension.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            _ => None,
        };
        if let Some(mime) = image_mime {
            if metadata.len() > 5 * 1024 * 1024 {
                bail!("image @{relative_display} exceeds the 5 MiB attachment limit");
            }
            let bytes = std::fs::read(&canonical)
                .with_context(|| format!("reading image @{relative_display}"))?;
            let data_url = format!("data:{mime};base64,{}", BASE64.encode(bytes));
            images.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": data_url }
            }));
            display.push_str(&format!("\n[Image: @{relative_display}]"));
        } else {
            if metadata.len() > 1024 * 1024 {
                bail!("text file @{relative_display} exceeds the 1 MiB attachment limit");
            }
            let contents = std::fs::read_to_string(&canonical)
                .with_context(|| format!("reading text file @{relative_display}"))?;
            text.push_str(&format!(
                "\n\n[Referenced file: @{relative_display}]\n```\n{contents}\n```"
            ));
            display.push_str(&format!("\n[File: @{relative_display}]"));
        }
    }

    Ok(provider::ChatMessage::user_with_images(
        display, text, images,
    ))
}

impl App {
    pub(super) fn set_workspace_trusted(&mut self, trusted: bool) -> Result<()> {
        let root = std::env::current_dir()?
            .canonicalize()
            .context("resolving workspace directory")?;
        let state_dir = root.join(".coolcode");
        let marker = state_dir.join("trusted");
        if trusted {
            std::fs::create_dir_all(&state_dir)
                .with_context(|| format!("creating {}", state_dir.display()))?;
            let canonical_state = state_dir
                .canonicalize()
                .with_context(|| format!("resolving {}", state_dir.display()))?;
            if !canonical_state.starts_with(&root) {
                bail!(
                    "refusing to write workspace trust state outside {}",
                    root.display()
                );
            }
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
            {
                Ok(mut file) => {
                    use std::io::Write as _;
                    file.write_all(b"trusted\n")?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !workspace_is_trusted(&root) {
                        bail!(
                            "the existing .coolcode/trusted marker resolves outside this workspace"
                        );
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("writing {}", marker.display()));
                }
            }
            self.workspace_trusted = true;
            self.notice = "Workspace trusted. Read, permission-controlled edit, and command tools are available.".to_owned();
        } else {
            if workspace_is_trusted(&root) {
                std::fs::remove_file(&marker)
                    .with_context(|| format!("removing {}", marker.display()))?;
            }
            self.workspace_trusted = false;
            self.notice =
                "Workspace trust revoked. COOL.md and @path file reads are disabled.".to_owned();
        }
        self.trust_prompt = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_is_trusted;

    #[test]
    fn workspace_trust_marker_is_scoped_to_its_folder() {
        let root =
            std::env::temp_dir().join(format!("coolcode-trust-test-{}", uuid::Uuid::new_v4()));
        let state_dir = root.join(".coolcode");
        std::fs::create_dir_all(&state_dir).expect("state directory");
        assert!(!workspace_is_trusted(&root));
        std::fs::write(state_dir.join("trusted"), "trusted\n").expect("marker");
        assert!(workspace_is_trusted(&root));
        std::fs::remove_dir_all(&root).expect("remove test workspace");
    }
}
