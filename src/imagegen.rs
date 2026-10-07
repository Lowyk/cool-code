//! Making placeholder images through an image API the user set up.
//!
//! This is off until the user adds an image API (a base URL, a model and a key) in Settings. The
//! key lives in the OS credential store, and the model only ever sees the `generate_image` tool
//! once all of that is in place. Calls go to an OpenAI-style `POST {base}/images/generations`.

use crate::Settings;
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read as _;
use std::time::Duration;

/// The name the image API's key is kept under in the OS credential store.
const KEY_NAME: &str = "image-generation";

#[cfg(not(test))]
pub(crate) fn key_name() -> String {
    KEY_NAME.to_owned()
}

// Tests run side by side and share one in-memory store, so each gets a key of its own.
#[cfg(test)]
pub(crate) fn key_name() -> String {
    format!("{KEY_NAME}-{:?}", std::thread::current().id())
}

/// The default endpoint offered when setting up.
pub(crate) const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// The default model offered when setting up.
pub(crate) const DEFAULT_MODEL: &str = "gpt-image-1";

/// The most image data accepted, however it arrives.
const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;
/// The longest prompt sent.
pub(crate) const MAX_PROMPT_CHARS: usize = 4_000;
const SQUARE: &str = "1024x1024";
const LANDSCAPE: &str = "1536x1024";
const PORTRAIT: &str = "1024x1536";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImageConfig {
    pub(crate) base_url: String,
    pub(crate) model: String,
}

/// The saved key, when there is one.
pub(crate) fn saved_key() -> Option<String> {
    crate::secrets::load(&key_name())
        .ok()
        .flatten()
        .filter(|key| !key.trim().is_empty())
}

/// Whether images can be made: an API is configured and its key is saved.
pub(crate) fn available(settings: &Settings) -> bool {
    settings
        .image_generation
        .as_ref()
        .is_some_and(|config| !config.base_url.trim().is_empty() && !config.model.trim().is_empty())
        && saved_key().is_some()
}

/// The size to request: a name, or an exact WIDTHxHEIGHT within sane limits.
pub(crate) fn resolve_size(requested: Option<&str>) -> Result<String> {
    let requested = requested.map(str::trim).unwrap_or("");
    match requested.to_ascii_lowercase().as_str() {
        "" | "square" => return Ok(SQUARE.to_owned()),
        "landscape" | "wide" => return Ok(LANDSCAPE.to_owned()),
        "portrait" | "tall" => return Ok(PORTRAIT.to_owned()),
        _ => {}
    }
    let exact = requested
        .to_ascii_lowercase()
        .split_once('x')
        .and_then(|(width, height)| {
            Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?))
        });
    match exact {
        Some((width, height))
            if (256..=4096).contains(&width) && (256..=4096).contains(&height) =>
        {
            Ok(format!("{width}x{height}"))
        }
        _ => bail!(
            "size must be square, landscape, portrait, or WIDTHxHEIGHT between 256 and 4096 (got `{requested}`)"
        ),
    }
}

/// Whether `path` ends in an image extension the tool saves.
pub(crate) fn check_extension(path: &str) -> Result<()> {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp") {
        Ok(())
    } else {
        bail!("the path must end in .png, .jpg, .jpeg or .webp (got `{path}`)")
    }
}

/// Where the image data is.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Bytes(Vec<u8>),
    Url(String),
}

/// Reads an images-API answer: inline base64 data, or a link to download.
pub(crate) fn parse_response(value: &Value) -> Result<Source> {
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        bail!("the image API said: {message}");
    }
    let first = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|data| data.first())
        .context("the image API returned no image")?;
    if let Some(encoded) = first.get("b64_json").and_then(Value::as_str) {
        let encoded = encoded
            .split_once(',')
            .filter(|(head, _)| head.starts_with("data:"))
            .map_or(encoded, |(_, rest)| rest);
        let bytes = BASE64
            .decode(encoded.trim())
            .context("the image API's data is not valid base64")?;
        return Ok(Source::Bytes(bytes));
    }
    if let Some(url) = first.get("url").and_then(Value::as_str) {
        return Ok(Source::Url(url.to_owned()));
    }
    bail!("the image API's answer had neither image data nor a link")
}

/// A link may be followed only if it is secure (or on this computer).
pub(crate) fn check_download_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("the image link is not a valid address")?;
    let local = matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    );
    if parsed.scheme() == "https" || (parsed.scheme() == "http" && local) {
        Ok(())
    } else {
        bail!("the image link is not secure (https), so it was not followed")
    }
}

/// Whether the data starts like a PNG, JPEG, WebP or GIF.
pub(crate) fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF8")
        || (bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("creating the HTTP client")
}

fn read_limited(response: reqwest::blocking::Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading the image")?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        bail!("the image is larger than the 25 MiB limit");
    }
    Ok(bytes)
}

/// Asks the image API for one image and returns its bytes.
pub(crate) fn generate(
    config: &ImageConfig,
    api_key: &str,
    prompt: &str,
    size: &str,
) -> Result<Vec<u8>> {
    // Same rules as every other endpoint that carries a key: secure, or on this computer.
    let endpoint = crate::endpoints::resolve_endpoint(&config.base_url, "images/generations")
        .map_err(|error| anyhow::anyhow!("the image API address is not usable: {error}"))?;
    let client = client()?;
    let response = client
        .post(&endpoint)
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": config.model,
            "prompt": prompt,
            "size": size,
            "n": 1,
        }))
        .send()
        .map_err(reqwest::Error::without_url)
        .context("sending the request to the image API")?;
    let status = response.status();
    if !status.is_success() {
        let body: String = response
            .text()
            .unwrap_or_default()
            .replace(api_key, "<key>")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect();
        bail!("the image API returned {status}: {body}");
    }
    let value: Value = response
        .json()
        .context("the image API did not answer with JSON")?;
    let bytes = match parse_response(&value)? {
        Source::Bytes(bytes) => bytes,
        Source::Url(url) => {
            check_download_url(&url)?;
            let download = client
                .get(&url)
                .send()
                .map_err(reqwest::Error::without_url)
                .context("downloading the image")?;
            if !download.status().is_success() {
                bail!("downloading the image returned {}", download.status());
            }
            read_limited(download)?
        }
    };
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        bail!("the image is larger than the 25 MiB limit");
    }
    if !looks_like_image(&bytes) {
        bail!("the image API returned something that is not an image");
    }
    Ok(bytes)
}

/// A hash of file contents, to recognize a file later.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest-of-a-tiny-png";

    fn config(base: &str) -> ImageConfig {
        ImageConfig {
            base_url: base.to_owned(),
            model: "test-image-model".to_owned(),
        }
    }

    #[test]
    fn sizes_have_friendly_names_and_exact_values_are_checked() {
        assert_eq!(resolve_size(None).unwrap(), "1024x1024");
        assert_eq!(resolve_size(Some("")).unwrap(), "1024x1024");
        assert_eq!(resolve_size(Some("Square")).unwrap(), "1024x1024");
        assert_eq!(resolve_size(Some("landscape")).unwrap(), "1536x1024");
        assert_eq!(resolve_size(Some("portrait")).unwrap(), "1024x1536");
        assert_eq!(resolve_size(Some("512X768")).unwrap(), "512x768");
        for bad in ["huge", "100x100", "9000x9000", "1024", "axb", "1024x"] {
            assert!(resolve_size(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_image_extensions_are_saved() {
        for good in ["a.png", "assets/hero.JPG", "x/y.jpeg", "z.webp"] {
            assert!(check_extension(good).is_ok(), "{good}");
        }
        for bad in ["a.txt", "a", "a.png.exe", "script.sh", ".png.", "dir/"] {
            assert!(check_extension(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_answer_may_carry_the_image_or_a_link_or_an_error() {
        let encoded = BASE64.encode(PNG);
        let inline = serde_json::json!({"data": [{"b64_json": encoded}]});
        assert_eq!(
            parse_response(&inline).unwrap(),
            Source::Bytes(PNG.to_vec())
        );
        let data_url =
            serde_json::json!({"data": [{"b64_json": format!("data:image/png;base64,{encoded}")}]});
        assert_eq!(
            parse_response(&data_url).unwrap(),
            Source::Bytes(PNG.to_vec())
        );
        let link = serde_json::json!({"data": [{"url": "https://cdn.example/a.png"}]});
        assert_eq!(
            parse_response(&link).unwrap(),
            Source::Url("https://cdn.example/a.png".to_owned())
        );
        let refused = serde_json::json!({"error": {"message": "content policy"}});
        assert!(format!("{:#}", parse_response(&refused).unwrap_err()).contains("content policy"));
        for broken in [
            serde_json::json!({}),
            serde_json::json!({"data": []}),
            serde_json::json!({"data": [{}]}),
            serde_json::json!({"data": [{"b64_json": "!!!not base64!!!"}]}),
        ] {
            assert!(parse_response(&broken).is_err(), "{broken}");
        }
    }

    #[test]
    fn a_link_is_followed_only_when_secure_or_local() {
        assert!(check_download_url("https://cdn.example/a.png").is_ok());
        assert!(check_download_url("http://localhost:9000/a.png").is_ok());
        for bad in [
            "http://cdn.example/a.png",
            "ftp://x/a.png",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(check_download_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn image_data_is_recognized_by_its_start() {
        assert!(looks_like_image(PNG));
        assert!(looks_like_image(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]));
        assert!(looks_like_image(b"GIF89a...."));
        assert!(looks_like_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!looks_like_image(b"<html>an error page</html>"));
        assert!(!looks_like_image(b""));
    }

    #[test]
    fn the_request_is_an_images_generation_call_with_the_key_and_the_inline_image_is_returned() {
        let body: &'static str = Box::leak(
            serde_json::json!({"data": [{"b64_json": BASE64.encode(PNG)}]})
                .to_string()
                .into_boxed_str(),
        );
        let (base, seen) = crate::testutil::serve_full(vec![(200, "application/json", body)]);
        let bytes =
            generate(&config(&base), "secret-key-1", "a red cube", "1536x1024").expect("image");
        assert_eq!(bytes, PNG);
        let requests = seen.lock().unwrap();
        let (head, sent) = &requests[0];
        assert!(head.starts_with("POST /images/generations "), "{head}");
        assert!(
            head.to_ascii_lowercase()
                .contains("authorization: bearer secret-key-1"),
            "{head}"
        );
        let sent: Value = serde_json::from_str(sent).unwrap();
        assert_eq!(sent["model"], "test-image-model");
        assert_eq!(sent["prompt"], "a red cube");
        assert_eq!(sent["size"], "1536x1024");
        assert_eq!(sent["n"], 1);
    }

    #[test]
    fn a_refusal_is_reported_without_the_key_and_a_non_image_is_rejected() {
        let (base, _) = crate::testutil::serve_full(vec![(
            401,
            "application/json",
            "{\"error\":{\"message\":\"bad key secret-key-2\"}}",
        )]);
        let error = generate(&config(&base), "secret-key-2", "x", "1024x1024").unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("401") && !text.contains("secret-key-2"),
            "{text}"
        );
        let html = serde_json::json!({"data": [{"b64_json": BASE64.encode("<html>oops</html>")}]})
            .to_string();
        let html: &'static str = Box::leak(html.into_boxed_str());
        let (base, _) = crate::testutil::serve_full(vec![(200, "application/json", html)]);
        let error = generate(&config(&base), "k", "x", "1024x1024").unwrap_err();
        assert!(format!("{error:#}").contains("not an image"));
    }

    #[test]
    fn an_insecure_address_is_refused_before_anything_is_sent() {
        let error =
            generate(&config("http://images.example/v1"), "k", "x", "1024x1024").unwrap_err();
        assert!(format!("{error:#}").contains("not usable"), "{error:#}");
    }

    #[test]
    fn images_are_available_only_with_both_the_settings_and_a_key() {
        let mut settings = Settings::default();
        assert!(!available(&settings));
        settings.image_generation = Some(config("https://api.example.com/v1"));
        let _ = crate::secrets::delete(&key_name());
        assert!(!available(&settings), "no key yet");
        crate::secrets::store(&key_name(), "a-key").unwrap();
        assert!(available(&settings));
        settings.image_generation = Some(ImageConfig {
            model: " ".to_owned(),
            ..config("https://api.example.com/v1")
        });
        assert!(!available(&settings), "a blank model is not a setup");
        crate::secrets::delete(&key_name()).unwrap();
    }

    #[test]
    fn a_hash_recognizes_the_same_bytes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(sha256_hex(b"abc"), sha256_hex(b"abd"));
    }
}
