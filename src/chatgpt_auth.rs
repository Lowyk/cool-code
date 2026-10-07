//! Signing in with a ChatGPT Plus/Pro account instead of an API key.
//!
//! **Unofficial.** This follows what other open-source coding tools do: an OAuth sign-in
//! (authorization code with PKCE) against OpenAI's account service using the public client id of
//! OpenAI's own Codex CLI, then requests to the ChatGPT backend that the Codex CLI uses. OpenAI
//! does not document this for third parties; it can change or be restricted at any time, and
//! using it may be subject to OpenAI's terms. Everything specific to that service is in the
//! constants below so it is easy to correct.
//!
//! Only the short refresh token (with the account id and email) is stored in the operating
//! system's credential store; the access token is a long JWT that does not fit comfortably in
//! every credential store, so it lives in memory and is renewed when needed.

use crate::login_page::{Outcome, Palette};
use crate::secrets;
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The public OAuth client id of OpenAI's Codex CLI.
pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub(crate) const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
pub(crate) const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// The redirect the client id is registered for; the port is fixed.
#[cfg_attr(test, allow(dead_code))]
pub(crate) const CALLBACK_PORT: u16 = 1455;
pub(crate) const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";
/// Where requests go once signed in.
pub(crate) const API_BASE: &str = "https://chatgpt.com/backend-api/codex";
/// The claim in the identity token that carries the ChatGPT account id.
const AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// How long to wait for the browser to come back.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Renew an access token this long before it expires.
const RENEW_BEFORE: i64 = 5 * 60;

/// What is remembered between runs (and is small enough for any credential store).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Account {
    pub(crate) refresh_token: String,
    pub(crate) account_id: Option<String>,
    pub(crate) email: Option<String>,
}

/// A usable access token and when it stops working (Unix seconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Session {
    pub(crate) access_token: String,
    pub(crate) expires_at: i64,
    pub(crate) account_id: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------------------------------------------------------------------------------------------
// PKCE and the authorization URL

/// A fresh code verifier and its S256 challenge.
pub(crate) fn new_pkce() -> (String, String) {
    let random = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let verifier = BASE64_URL.encode(random.as_bytes());
    let challenge = challenge_for(&verifier);
    (verifier, challenge)
}

pub(crate) fn challenge_for(verifier: &str) -> String {
    BASE64_URL.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) fn authorize_url(challenge: &str, state: &str) -> String {
    let query = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("scope", SCOPE),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "codex_cli_rs"),
    ]
    .iter()
    .map(|(key, value)| format!("{key}={}", encode(value)))
    .collect::<Vec<_>>()
    .join("&");
    format!("{AUTHORIZE_URL}?{query}")
}

/// Percent-encodes everything outside the unreserved set.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The browser round trip

/// A sign-in in progress: the page to open, and the local server waiting for the redirect.
pub(crate) struct LoginFlow {
    pub(crate) url: String,
    listener: TcpListener,
    state: String,
    verifier: String,
    /// The colors of the page the browser lands on.
    palette: Palette,
}

impl LoginFlow {
    /// Binds the callback port and builds the page to open.
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn start() -> Result<LoginFlow> {
        Self::start_on(&format!("127.0.0.1:{CALLBACK_PORT}"))
    }

    pub(crate) fn start_on(address: &str) -> Result<LoginFlow> {
        let listener = TcpListener::bind(address).with_context(|| {
            format!(
                "could not listen on {address} for the sign-in redirect (is another sign-in, or another coding tool, using it?)"
            )
        })?;
        listener
            .set_nonblocking(true)
            .context("preparing the local sign-in listener")?;
        let (verifier, challenge) = new_pkce();
        let state = uuid::Uuid::new_v4().simple().to_string();
        Ok(LoginFlow {
            url: authorize_url(&challenge, &state),
            listener,
            state,
            verifier,
            palette: Palette::default(),
        })
    }

    /// Draws the page the browser lands on in these colors.
    pub(crate) fn with_palette(mut self, palette: Palette) -> LoginFlow {
        self.palette = palette;
        self
    }

    /// Waits for the browser to return, then trades the code for tokens.
    pub(crate) fn finish(self, token_url: &str, cancel: &AtomicBool) -> Result<(Account, Session)> {
        let code = wait_for_code_with(
            &self.listener,
            &self.state,
            cancel,
            LOGIN_TIMEOUT,
            &self.palette,
        )?;
        exchange_code(token_url, &code, &self.verifier)
    }
}

/// Accepts connections until one carries the redirect, answers it with a short page, and returns
/// the authorization code. Connections that are not the redirect get a 404 and are ignored.
#[cfg(test)]
fn wait_for_code(
    listener: &TcpListener,
    expected_state: &str,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String> {
    wait_for_code_with(
        listener,
        expected_state,
        cancel,
        timeout,
        &Palette::default(),
    )
}

fn wait_for_code_with(
    listener: &TcpListener,
    expected_state: &str,
    cancel: &AtomicBool,
    timeout: Duration,
    palette: &Palette,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for the browser sign-in to finish");
        }
        let mut connection = match listener.accept() {
            Ok((connection, _)) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(error) => return Err(error).context("accepting the sign-in redirect"),
        };
        connection.set_nonblocking(false).ok();
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .ok();
        let mut first_line = String::new();
        if BufReader::new(&connection)
            .read_line(&mut first_line)
            .is_err()
        {
            continue;
        }
        let target = first_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .to_owned();
        let parsed = reqwest::Url::parse(&format!("http://localhost{target}")).ok();
        let Some(url) = parsed.filter(|url| url.path() == "/auth/callback") else {
            reply(
                &mut connection,
                palette,
                404,
                "Not found",
                Outcome::Failure,
                "Nothing here",
                "This address is only used to finish a sign-in.",
            );
            continue;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if let Some(error) = param("error") {
            let detail = param("error_description").unwrap_or_default();
            reply(
                &mut connection,
                palette,
                200,
                "Sign-in failed",
                Outcome::Failure,
                "Sign-in did not finish",
                "ChatGPT did not complete the sign-in. You can close this tab and try again from your terminal.",
            );
            bail!("the sign-in was refused: {error} {detail}");
        }
        if param("state").as_deref() != Some(expected_state) {
            reply(
                &mut connection,
                palette,
                400,
                "Bad request",
                Outcome::Failure,
                "Sign-in was ignored",
                "This sign-in did not start from Cool Code, so it was not used. Close this tab and try again from your terminal.",
            );
            bail!("the sign-in redirect had the wrong state, so it was ignored");
        }
        let Some(code) = param("code").filter(|code| !code.is_empty()) else {
            reply(
                &mut connection,
                palette,
                400,
                "Bad request",
                Outcome::Failure,
                "Sign-in did not finish",
                "No sign-in code came back. Close this tab and try again from your terminal.",
            );
            bail!("the sign-in redirect carried no authorization code");
        };
        reply(
            &mut connection,
            palette,
            200,
            "Signed in",
            Outcome::Success,
            "You are signed in",
            "Cool Code received your sign-in. You can close this tab and go back to your terminal.",
        );
        return Ok(code);
    }
}

fn reply(
    connection: &mut std::net::TcpStream,
    palette: &Palette,
    status: u16,
    reason: &str,
    outcome: Outcome,
    headline: &str,
    detail: &str,
) {
    let body = crate::login_page::page(outcome, headline, detail, palette);
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = connection.write_all(response.as_bytes());
}

// ---------------------------------------------------------------------------------------------
// Tokens

fn token_request(token_url: &str, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("creating the HTTP client")?;
    let response = client
        .post(token_url)
        .form(form)
        .send()
        .context("contacting the sign-in service")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        bail!(
            "the sign-in service answered {status}: {}",
            body.chars().take(300).collect::<String>()
        );
    }
    response
        .json::<TokenResponse>()
        .context("reading the sign-in service's answer")
}

/// The account id and email claims in an identity token. The token comes straight from the
/// sign-in service over TLS, so its signature is not checked here; nothing trusts these values
/// for access control, they only label requests and the UI.
pub(crate) fn claims(id_token: &str) -> (Option<String>, Option<String>) {
    let payload = id_token.split('.').nth(1).unwrap_or("");
    let Ok(bytes) = BASE64_URL.decode(payload.trim_end_matches('=')) else {
        return (None, None);
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return (None, None);
    };
    let account = claims
        .get(AUTH_CLAIM)
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let email = claims
        .get("email")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    (account, email)
}

fn into_pair(response: TokenResponse, previous: Option<&Account>) -> Result<(Account, Session)> {
    let refresh_token = response
        .refresh_token
        .or_else(|| previous.map(|account| account.refresh_token.clone()))
        .context("the sign-in service returned no refresh token")?;
    let (account_id, email) = response
        .id_token
        .as_deref()
        .map(claims)
        .unwrap_or((None, None));
    let account = Account {
        refresh_token,
        account_id: account_id.or_else(|| previous.and_then(|p| p.account_id.clone())),
        email: email.or_else(|| previous.and_then(|p| p.email.clone())),
    };
    let session = Session {
        access_token: response.access_token,
        expires_at: now() + response.expires_in.unwrap_or(3600),
        account_id: account.account_id.clone(),
    };
    Ok((account, session))
}

pub(crate) fn exchange_code(
    token_url: &str,
    code: &str,
    verifier: &str,
) -> Result<(Account, Session)> {
    let response = token_request(
        token_url,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ],
    )?;
    into_pair(response, None)
}

fn refresh(token_url: &str, account: &Account) -> Result<(Account, Session)> {
    let response = token_request(
        token_url,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &account.refresh_token),
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
        ],
    )?;
    into_pair(response, Some(account))
}

// ---------------------------------------------------------------------------------------------
// Keeping the account

static SESSIONS: Mutex<Option<HashMap<String, Session>>> = Mutex::new(None);
/// Refresh tokens may be single-use, so two threads must never renew at once.
static RENEWING: Mutex<()> = Mutex::new(());

fn cached(provider_id: &str) -> Option<Session> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .and_then(|sessions| sessions.get(provider_id).cloned())
}

fn remember(provider_id: &str, session: Session) {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(provider_id.to_owned(), session);
}

fn usable(session: &Session) -> bool {
    session.expires_at - now() > RENEW_BEFORE
}

pub(crate) fn save_account(provider_id: &str, account: &Account) -> Result<()> {
    secrets::store(
        provider_id,
        &serde_json::to_string(account).context("serializing the account")?,
    )
}

pub(crate) fn load_account(provider_id: &str) -> Result<Option<Account>> {
    let Some(stored) = secrets::load(provider_id)? else {
        return Ok(None);
    };
    serde_json::from_str(&stored)
        .map(Some)
        .context("the saved ChatGPT sign-in is unreadable; sign in again")
}

/// Records a completed sign-in.
pub(crate) fn remember_login(provider_id: &str, account: &Account, session: Session) -> Result<()> {
    save_account(provider_id, account)?;
    remember(provider_id, session);
    Ok(())
}

/// A working access token for `provider_id`, renewed (and the new refresh token saved) if the
/// current one is missing or about to expire.
pub(crate) fn session_for(provider_id: &str, token_url: &str) -> Result<Session> {
    if let Some(session) = cached(provider_id).filter(usable) {
        return Ok(session);
    }
    let _renewing = RENEWING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Another thread may have renewed while this one waited.
    if let Some(session) = cached(provider_id).filter(usable) {
        return Ok(session);
    }
    let account = load_account(provider_id)?
        .context("not signed in to ChatGPT; sign in from the Providers settings")?;
    let (renewed, session) = refresh(token_url, &account).context(
        "renewing the ChatGPT sign-in failed; sign in again from the Providers settings",
    )?;
    save_account(provider_id, &renewed)?;
    remember(provider_id, session.clone());
    Ok(session)
}

/// The Codex version the backend is told this client is. It hides models from clients it
/// considers too old, so this must stay recent; `COOLCODE_CODEX_CLIENT_VERSION` overrides it.
const CLIENT_VERSION: &str = "0.160.1";

/// Where the plan's usage windows are reported.
pub(crate) const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// Where the account's models are listed.
pub(crate) const MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";

fn client_version() -> String {
    std::env::var("COOLCODE_CODEX_CLIENT_VERSION")
        .ok()
        .filter(|version| !version.trim().is_empty())
        .unwrap_or_else(|| CLIENT_VERSION.to_owned())
}

/// A signed-in GET, answered as JSON.
fn get_json(
    provider_id: &str,
    url: &str,
    token_url: &str,
    what: &str,
) -> Result<serde_json::Value> {
    let session = session_for(provider_id, token_url)?;
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        // The same identity as the `originator` header below.
        .user_agent(format!("codex_cli_rs/{}", client_version()))
        .build()
        .context("creating HTTP client")?;
    let mut request = client
        .get(url)
        .bearer_auth(&session.access_token)
        .header("originator", "codex_cli_rs")
        .header("Accept", "application/json");
    if let Some(account) = &session.account_id {
        request = request.header("chatgpt-account-id", account);
    }
    let response = request
        .send()
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("asking ChatGPT for {what}"))?;
    let status = response.status();
    if !status.is_success() {
        // The reason a request was refused is in the body, which never holds a credential.
        let reason: String = response
            .text()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect();
        if reason.is_empty() {
            bail!("ChatGPT returned {status} when asked for {what}");
        }
        bail!("ChatGPT returned {status} when asked for {what}: {reason}");
    }
    response
        .json()
        .with_context(|| format!("ChatGPT did not return {what} as JSON"))
}

/// The models the signed-in account can use, as the backend's raw JSON.
pub(crate) fn fetch_models(
    provider_id: &str,
    models_url: &str,
    token_url: &str,
) -> Result<serde_json::Value> {
    let versioned = format!("{models_url}?client_version={}", encode(&client_version()));
    get_json(provider_id, &versioned, token_url, "its models")
}

/// The plan's usage windows, as the backend's raw JSON.
pub(crate) fn fetch_usage(
    provider_id: &str,
    usage_url: &str,
    token_url: &str,
) -> Result<serde_json::Value> {
    get_json(provider_id, usage_url, token_url, "its usage")
}

/// Forgets a sign-in everywhere.
pub(crate) fn sign_out(provider_id: &str) -> Result<()> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_or_insert_with(HashMap::new)
        .remove(provider_id);
    secrets::delete(provider_id)
}

/// Opens `url` in the user's browser. Failure is not an error: the URL is also shown on screen.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn open_browser(url: &str) -> bool {
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::serve;
    use std::io::Read;

    fn jwt(claims: &serde_json::Value) -> String {
        format!(
            "{}.{}.sig",
            BASE64_URL.encode(b"{}"),
            BASE64_URL.encode(claims.to_string())
        )
    }

    fn token_json(
        access: &str,
        refresh: Option<&str>,
        id_claims: Option<&serde_json::Value>,
    ) -> &'static str {
        let mut value = serde_json::json!({"access_token": access, "expires_in": 3600});
        if let Some(refresh) = refresh {
            value["refresh_token"] = refresh.into();
        }
        if let Some(claims) = id_claims {
            value["id_token"] = jwt(claims).into();
        }
        Box::leak(value.to_string().into_boxed_str())
    }

    #[test]
    fn pkce_challenges_are_the_sha256_of_the_verifier() {
        // The RFC 7636 example.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let (verifier, challenge) = new_pkce();
        assert!((43..=128).contains(&verifier.len()), "{}", verifier.len());
        assert_eq!(challenge_for(&verifier), challenge);
        assert_ne!(new_pkce().0, verifier, "every sign-in has its own verifier");
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn the_authorize_url_carries_everything_the_service_needs() {
        let url = reqwest::Url::parse(&authorize_url("CHALLENGE", "STATE")).expect("url");
        assert_eq!(
            url.origin().ascii_serialization(),
            "https://auth.openai.com"
        );
        assert_eq!(url.path(), "/oauth/authorize");
        let get = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        assert_eq!(get("response_type").as_deref(), Some("code"));
        assert_eq!(get("client_id").as_deref(), Some(CLIENT_ID));
        assert_eq!(get("redirect_uri").as_deref(), Some(REDIRECT_URI));
        assert_eq!(get("code_challenge").as_deref(), Some("CHALLENGE"));
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(get("state").as_deref(), Some("STATE"));
        assert!(get("scope").unwrap().contains("offline_access"));
    }

    #[test]
    fn claims_are_read_from_the_identity_token() {
        let token = jwt(&serde_json::json!({
            "email": "me@example.com",
            AUTH_CLAIM: {"chatgpt_account_id": "acct-123"}
        }));
        assert_eq!(
            claims(&token),
            (
                Some("acct-123".to_owned()),
                Some("me@example.com".to_owned())
            )
        );
        assert_eq!(claims("garbage"), (None, None));
        assert_eq!(claims("a.b.c"), (None, None));
        assert_eq!(claims(&jwt(&serde_json::json!({"other": 1}))), (None, None));
    }

    #[test]
    fn the_code_is_exchanged_with_the_verifier_and_the_answer_is_understood() {
        let claims = serde_json::json!({"email": "me@example.com", AUTH_CLAIM: {"chatgpt_account_id": "acct-9"}});
        let (url, seen) = serve(vec![(
            200,
            "application/json",
            token_json("access-1", Some("refresh-1"), Some(&claims)),
        )]);
        let (account, session) =
            exchange_code(&format!("{url}/token"), "the-code", "the-verifier").expect("exchange");
        assert_eq!(account.refresh_token, "refresh-1");
        assert_eq!(account.account_id.as_deref(), Some("acct-9"));
        assert_eq!(account.email.as_deref(), Some("me@example.com"));
        assert_eq!(session.access_token, "access-1");
        assert!(session.expires_at > now() + 3000);
        let body = seen.lock().unwrap()[0].clone();
        for part in [
            "grant_type=authorization_code",
            "code=the-code",
            "code_verifier=the-verifier",
            &format!("client_id={CLIENT_ID}"),
            "redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback",
        ] {
            assert!(body.contains(part), "{part} missing from {body}");
        }
    }

    #[test]
    fn a_refused_exchange_explains_itself_without_leaking_secrets() {
        let (url, _) = serve(vec![(
            400,
            "application/json",
            "{\"error\":\"invalid_grant\"}",
        )]);
        let error = exchange_code(&format!("{url}/token"), "bad", "v").unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("400") && text.contains("invalid_grant"),
            "{text}"
        );
        assert!(!text.contains("verifier"), "{text}");
    }

    fn browser_returns(
        flow_state: &str,
        target: &str,
        port_listener: &TcpListener,
    ) -> std::thread::JoinHandle<String> {
        let address = port_listener.local_addr().unwrap();
        let target = target.replace("STATE", flow_state);
        std::thread::spawn(move || {
            let mut connection = std::net::TcpStream::connect(address).expect("connect");
            write!(
                connection,
                "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            )
            .unwrap();
            let mut answer = String::new();
            connection.read_to_string(&mut answer).ok();
            answer
        })
    }

    #[test]
    fn the_redirect_is_accepted_when_the_state_matches() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let browser = browser_returns("abc", "/auth/callback?code=CODE%2B1&state=STATE", &listener);
        let cancel = AtomicBool::new(false);
        let code = wait_for_code(&listener, "abc", &cancel, Duration::from_secs(5)).expect("code");
        assert_eq!(code, "CODE+1", "the code is percent-decoded");
        let page = browser.join().unwrap();
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("You are signed in"), "{page}");
    }

    #[test]
    fn the_page_the_browser_gets_is_drawn_in_the_palette_the_flow_was_given() {
        let palette = Palette {
            window: [11, 22, 33],
            ..Palette::default()
        };
        let flow = LoginFlow::start_on("127.0.0.1:0")
            .expect("flow")
            .with_palette(palette);
        let browser = browser_returns("abc", "/auth/callback?code=c&state=STATE", &flow.listener);
        let cancel = AtomicBool::new(false);
        wait_for_code_with(
            &flow.listener,
            "abc",
            &cancel,
            Duration::from_secs(5),
            &flow.palette,
        )
        .expect("code");
        let page = browser.join().unwrap();
        assert!(page.contains("rgb(11,22,33)"), "{page}");
        assert!(page.contains("cool-code sign-in"), "{page}");
    }

    #[test]
    fn a_redirect_with_the_wrong_state_or_an_error_is_rejected() {
        for (target, expected) in [
            ("/auth/callback?code=x&state=WRONG", "wrong state"),
            ("/auth/callback?state=STATE", "no authorization code"),
            ("/auth/callback?error=access_denied&state=STATE", "refused"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let browser = browser_returns("abc", target, &listener);
            let cancel = AtomicBool::new(false);
            let error =
                wait_for_code(&listener, "abc", &cancel, Duration::from_secs(5)).unwrap_err();
            assert!(
                format!("{error:#}").contains(expected),
                "{target}: {error:#}"
            );
            browser.join().unwrap();
        }
    }

    #[test]
    fn other_requests_are_ignored_until_the_real_redirect_arrives() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let stray = browser_returns("abc", "/favicon.ico", &listener);
        let cancel = AtomicBool::new(false);
        let waiter = {
            let listener = listener.try_clone().unwrap();
            std::thread::spawn(move || {
                wait_for_code(
                    &listener,
                    "abc",
                    &AtomicBool::new(false),
                    Duration::from_secs(5),
                )
            })
        };
        assert!(stray.join().unwrap().starts_with("HTTP/1.1 404"));
        let real = browser_returns("abc", "/auth/callback?code=OK&state=STATE", &listener);
        assert_eq!(waiter.join().unwrap().expect("code"), "OK");
        real.join().unwrap();
        let _ = cancel;
    }

    #[test]
    fn waiting_can_be_cancelled_and_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let cancelled = AtomicBool::new(true);
        let error = wait_for_code(&listener, "s", &cancelled, Duration::from_secs(5)).unwrap_err();
        assert!(format!("{error:#}").contains("cancelled"));
        let waiting = AtomicBool::new(false);
        let error =
            wait_for_code(&listener, "s", &waiting, Duration::from_millis(150)).unwrap_err();
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
    }

    #[test]
    fn starting_a_sign_in_while_the_port_is_taken_says_so() {
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = taken.local_addr().unwrap().to_string();
        let error = LoginFlow::start_on(&address).err().expect("busy");
        assert!(
            format!("{error:#}").contains("could not listen"),
            "{error:#}"
        );
    }

    #[test]
    fn a_whole_sign_in_ends_with_a_saved_account() {
        let claims = serde_json::json!({"email": "me@example.com", AUTH_CLAIM: {"chatgpt_account_id": "acct-1"}});
        let (token_url, seen) = serve(vec![(
            200,
            "application/json",
            token_json("access-A", Some("refresh-A"), Some(&claims)),
        )]);
        let flow = LoginFlow::start_on("127.0.0.1:0").expect("flow");
        let address = flow.listener.local_addr().unwrap();
        let url = reqwest::Url::parse(&flow.url).unwrap();
        let state = url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let verifier_challenge = url
            .query_pairs()
            .find(|(k, _)| k == "code_challenge")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(challenge_for(&flow.verifier), verifier_challenge);
        let browser = std::thread::spawn(move || {
            let mut c = std::net::TcpStream::connect(address).unwrap();
            write!(
                c,
                "GET /auth/callback?code=THECODE&state={state} HTTP/1.1\r\n\r\n"
            )
            .unwrap();
            let mut sink = String::new();
            c.read_to_string(&mut sink).ok();
        });
        let (account, session) = flow
            .finish(&format!("{token_url}/token"), &AtomicBool::new(false))
            .expect("signed in");
        browser.join().unwrap();
        assert_eq!(account.refresh_token, "refresh-A");
        assert_eq!(session.account_id.as_deref(), Some("acct-1"));
        assert!(seen.lock().unwrap()[0].contains("code=THECODE"));
        let id = format!("chatgpt-test-{}", uuid::Uuid::new_v4());
        remember_login(&id, &account, session).expect("saved");
        assert_eq!(load_account(&id).unwrap(), Some(account));
        sign_out(&id).expect("signed out");
        assert_eq!(load_account(&id).unwrap(), None);
        assert!(cached(&id).is_none());
    }

    #[test]
    fn a_valid_session_is_reused_and_an_expiring_one_is_renewed_and_saved() {
        let id = format!("chatgpt-test-{}", uuid::Uuid::new_v4());
        let account = Account {
            refresh_token: "refresh-old".to_owned(),
            account_id: Some("acct".to_owned()),
            email: None,
        };
        save_account(&id, &account).unwrap();
        // A fresh session needs no network at all.
        remember(
            &id,
            Session {
                access_token: "still-good".to_owned(),
                expires_at: now() + 3600,
                account_id: None,
            },
        );
        assert_eq!(
            session_for(&id, "http://127.0.0.1:1/unused")
                .unwrap()
                .access_token,
            "still-good"
        );
        // One about to expire is renewed, and the rotated refresh token is kept.
        remember(
            &id,
            Session {
                access_token: "stale".to_owned(),
                expires_at: now() + 30,
                account_id: None,
            },
        );
        let (url, seen) = serve(vec![(
            200,
            "application/json",
            token_json("access-new", Some("refresh-new"), None),
        )]);
        let renewed = session_for(&id, &format!("{url}/token")).expect("renewed");
        assert_eq!(renewed.access_token, "access-new");
        assert_eq!(
            renewed.account_id.as_deref(),
            Some("acct"),
            "kept from before"
        );
        assert!(seen.lock().unwrap()[0].contains("grant_type=refresh_token"));
        assert!(seen.lock().unwrap()[0].contains("refresh_token=refresh-old"));
        assert_eq!(
            load_account(&id).unwrap().unwrap().refresh_token,
            "refresh-new"
        );
        assert_eq!(
            session_for(&id, "http://127.0.0.1:1/unused")
                .unwrap()
                .access_token,
            "access-new"
        );
        sign_out(&id).unwrap();
    }

    #[test]
    fn without_an_account_or_when_renewal_fails_the_message_says_what_to_do() {
        let id = format!("chatgpt-test-{}", uuid::Uuid::new_v4());
        let error = session_for(&id, "http://127.0.0.1:1/unused").unwrap_err();
        assert!(format!("{error:#}").contains("not signed in"), "{error:#}");
        save_account(
            &id,
            &Account {
                refresh_token: "r".to_owned(),
                account_id: None,
                email: None,
            },
        )
        .unwrap();
        let (url, _) = serve(vec![(
            401,
            "application/json",
            "{\"error\":\"invalid_grant\"}",
        )]);
        let error = session_for(&id, &format!("{url}/token")).unwrap_err();
        assert!(format!("{error:#}").contains("sign in again"), "{error:#}");
        sign_out(&id).unwrap();
    }

    #[test]
    fn listing_models_sends_the_account_headers_and_the_client_version() {
        let (base, seen) = crate::testutil::serve_full(vec![(
            200,
            "application/json",
            "{\"models\":[{\"slug\":\"gpt-6.1\"}]}",
        )]);
        remember_login(
            "models-listing",
            &Account {
                refresh_token: "r".to_owned(),
                account_id: Some("acct-9".to_owned()),
                email: None,
            },
            Session {
                access_token: "tok-9".to_owned(),
                expires_at: now() + 3600,
                account_id: Some("acct-9".to_owned()),
            },
        )
        .unwrap();
        let value = fetch_models(
            "models-listing",
            &format!("{base}/models"),
            "http://unused.invalid/token",
        )
        .unwrap();
        assert_eq!(value["models"][0]["slug"], "gpt-6.1");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let head = seen[0].0.to_ascii_lowercase();
        assert!(head.starts_with("get /models?client_version="), "{head}");
        assert!(head.contains("authorization: bearer tok-9"), "{head}");
        assert!(head.contains("chatgpt-account-id: acct-9"), "{head}");
    }

    #[test]
    fn a_refused_listing_reports_the_reason_the_server_gave() {
        let (base, _) = crate::testutil::serve_full(vec![(
            400,
            "application/json",
            "{\"detail\": \"client_version is not valid\"}",
        )]);
        remember_login(
            "models-refused",
            &Account {
                refresh_token: "r".to_owned(),
                account_id: None,
                email: None,
            },
            Session {
                access_token: "tok-refused".to_owned(),
                expires_at: now() + 3600,
                account_id: None,
            },
        )
        .unwrap();
        let error = fetch_models(
            "models-refused",
            &format!("{base}/models"),
            "http://unused.invalid/token",
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("400") && text.contains("client_version is not valid"),
            "{text}"
        );
        assert!(!text.contains("tok-refused"), "{text}");
    }
}
