//! Which lab made a model, for grouping long model lists under provider aggregators.

/// The display name of the lab behind `model_id`, judged from its name or its `author/` prefix.
pub(super) fn creator_of(model_id: &str) -> String {
    let lower = model_id.to_ascii_lowercase();
    let (author, name) = match lower.split_once('/') {
        Some((author, name)) => (Some(author), name),
        None => (None, lower.as_str()),
    };
    let families: [(&[&str], &str); 12] = [
        (&["claude"], "Claude"),
        (&["gemini", "gemma"], "Google Gemini"),
        (&["gpt", "chatgpt", "o1", "o3", "o4"], "OpenAI"),
        (&["kimi", "moonshot"], "Moonshot"),
        (&["qwen", "qwq"], "Qwen"),
        (&["deepseek"], "DeepSeek"),
        (&["glm"], "Z.ai"),
        (&["llama"], "Meta Llama"),
        (&["mistral", "mixtral", "codestral", "ministral"], "Mistral"),
        (&["grok"], "xAI"),
        (&["minimax"], "MiniMax"),
        (&["command"], "Cohere"),
    ];
    for (prefixes, label) in families {
        if prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            return (*label).to_owned();
        }
    }
    match author {
        Some(author) if !author.is_empty() => {
            let mut chars = author.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
        _ => "Other".to_owned(),
    }
}

/// The creator a built-in provider *is*, when the provider only serves its own models.
pub(super) fn native_creator(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "google" => Some("Google Gemini"),
        "anthropic" => Some("Claude"),
        "openai" => Some("OpenAI"),
        _ => None,
    }
}

/// Drops a leading family word the group heading already says: "Claude Opus 5.5" under
/// "Claude" reads "Opus 5.5".
pub(super) fn strip_group_word(name: &str, group: &str) -> String {
    match name.split_once(' ') {
        Some((first, rest))
            if group
                .to_ascii_lowercase()
                .contains(&first.to_ascii_lowercase()) =>
        {
            rest.to_owned()
        }
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{creator_of, native_creator, strip_group_word};

    #[test]
    fn models_are_attributed_by_name_or_author() {
        assert_eq!(creator_of("claude-opus-5-5"), "Claude");
        assert_eq!(creator_of("anthropic/claude-opus-5"), "Claude");
        assert_eq!(creator_of("gemini-3-flash"), "Google Gemini");
        assert_eq!(creator_of("gpt-6-astra"), "OpenAI");
        assert_eq!(creator_of("openai/gpt-oss-120b"), "OpenAI");
        assert_eq!(creator_of("o3-mini"), "OpenAI");
        assert_eq!(creator_of("kimi-k2.7-code"), "Moonshot");
        assert_eq!(creator_of("qwen/qwen3.8-27b"), "Qwen");
        assert_eq!(creator_of("meta-llama/llama-3-70b-instruct"), "Meta Llama");
        assert_eq!(creator_of("nvidia/nemotron-4"), "Nvidia");
        assert_eq!(creator_of("mystery"), "Other");
    }

    #[test]
    fn only_first_party_providers_are_their_own_creator() {
        assert_eq!(native_creator("google"), Some("Google Gemini"));
        assert_eq!(native_creator("anthropic"), Some("Claude"));
        assert_eq!(native_creator("openai"), Some("OpenAI"));
        assert_eq!(native_creator("multiai"), None);
        assert_eq!(native_creator("anthropic-custom"), None);
    }

    #[test]
    fn a_family_word_the_heading_repeats_is_dropped() {
        assert_eq!(strip_group_word("Claude Opus 5.5", "Claude"), "Opus 5.5");
        assert_eq!(
            strip_group_word("Gemini 3 Flash", "Google Gemini"),
            "3 Flash"
        );
        assert_eq!(strip_group_word("GPT-6 Astra", "OpenAI"), "GPT-6 Astra");
        assert_eq!(strip_group_word("Kimi K3", "Moonshot"), "Kimi K3");
        assert_eq!(strip_group_word("Qwen3.8-Max", "Qwen"), "Qwen3.8-Max");
    }
}
