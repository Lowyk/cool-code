//! The page a browser lands on after a sign-in, served by the local redirect server.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Success,
    Failure,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A small self-contained page (no scripts, no external requests) that follows the system's
/// light or dark setting.
pub(crate) fn page(outcome: Outcome, headline: &str, detail: &str) -> String {
    let (mark, accent) = match outcome {
        Outcome::Success => ("&#10003;", "#3fb68b"),
        Outcome::Failure => ("&#10005;", "#e5534b"),
    };
    format!(
        "<!doctype html><html lang=en><meta charset=utf-8>\
<meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>Cool Code</title>\
<style>\
:root{{--bg:#f5f7fa;--card:#fff;--text:#1c2430;--dim:#5b6675;--accent:{accent}}}\
@media(prefers-color-scheme:dark){{:root{{--bg:#11161d;--card:#1a212b;--text:#e6ebf2;--dim:#93a0b1}}}}\
*{{box-sizing:border-box}}\
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--text);font:16px/1.5 system-ui,-apple-system,Segoe UI,sans-serif}}\
main{{width:min(26rem,calc(100% - 2rem));padding:2.5rem 2rem;border-radius:1rem;background:var(--card);text-align:center;box-shadow:0 .5rem 2rem rgba(0,0,0,.18)}}\
.mark{{width:3.5rem;height:3.5rem;margin:0 auto 1.25rem;border-radius:50%;display:grid;place-items:center;font-size:1.6rem;color:#fff;background:var(--accent)}}\
h1{{margin:0 0 .5rem;font-size:1.35rem}}\
p{{margin:0;color:var(--dim)}}\
.brand{{margin-top:1.75rem;font-size:.8rem;letter-spacing:.08em;text-transform:uppercase;color:var(--dim)}}\
</style>\
<main><div class=mark>{mark}</div><h1>{headline}</h1><p>{detail}</p><div class=brand>Cool Code</div></main>",
        headline = escape(headline),
        detail = escape(detail),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_success_page_says_so_and_tells_you_to_go_back() {
        let html = page(
            Outcome::Success,
            "You are signed in",
            "Return to your terminal.",
        );
        assert!(html.contains("You are signed in") && html.contains("Return to your terminal."));
        assert!(html.contains("&#10003;") && html.contains("#3fb68b"));
        assert!(html.contains("prefers-color-scheme:dark"));
        assert!(!html.contains("<script"), "no scripts");
    }

    #[test]
    fn a_failure_page_looks_different_and_text_is_escaped() {
        let html = page(
            Outcome::Failure,
            "Sign-in did not finish",
            "<b>access_denied</b> & more",
        );
        assert!(html.contains("&#10005;") && html.contains("#e5534b"));
        assert!(
            html.contains("&lt;b&gt;access_denied&lt;/b&gt; &amp; more"),
            "{html}"
        );
        assert!(!html.contains("<b>access_denied"), "{html}");
    }
}
