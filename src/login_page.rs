//! The page a browser lands on after a sign-in, served by the local redirect server.
//!
//! It looks like a Windows 10 console window (dark, square corners, a flat title bar with the
//! minimize, maximize and close buttons) in the colors of the user's theme, so the browser tab
//! feels like part of Cool Code.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Success,
    Failure,
}

/// The colors a page is drawn with, as red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    /// Behind the window.
    pub(crate) background: [u8; 3],
    /// The window itself.
    pub(crate) window: [u8; 3],
    /// Main text.
    pub(crate) text: [u8; 3],
    /// Secondary text and the window's border.
    pub(crate) dim: [u8; 3],
    /// The prompt, the title and the cursor.
    pub(crate) accent: [u8; 3],
    /// A successful result.
    pub(crate) good: [u8; 3],
    /// A failed result.
    pub(crate) bad: [u8; 3],
}

impl Default for Palette {
    /// The default (Cool) theme.
    fn default() -> Self {
        Palette {
            background: [22, 24, 27],
            window: [25, 32, 38],
            text: [226, 232, 240],
            dim: [128, 158, 184],
            accent: [98, 213, 244],
            good: [98, 213, 244],
            bad: [235, 80, 80],
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn css(color: [u8; 3]) -> String {
    format!("rgb({},{},{})", color[0], color[1], color[2])
}

/// Darkens a color towards black by `amount` (0 keeps it, 1 is black).
fn darker(color: [u8; 3], amount: f32) -> [u8; 3] {
    color.map(|channel| (f32::from(channel) * (1.0 - amount)).round() as u8)
}

/// A small self-contained page (no scripts, no external requests) drawn like a Windows 10
/// console window. Each line of `detail` becomes a line of output.
pub(crate) fn page(outcome: Outcome, headline: &str, detail: &str, palette: &Palette) -> String {
    let (mark, result) = match outcome {
        Outcome::Success => ("&#10003;", palette.good),
        Outcome::Failure => ("&#10005;", palette.bad),
    };
    let output = detail
        .lines()
        .map(|line| format!("<div class=out>  {}</div>", escape(line)))
        .collect::<String>();
    format!(
        "<!doctype html><html lang=en><meta charset=utf-8>\
<meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>Cool Code</title>\
<style>\
:root{{--bg:{bg};--window:{window};--titlebar:{titlebar};--text:{text};--dim:{dim};--accent:{accent};--result:{result}}}\
*{{box-sizing:border-box}}\
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--text);\
font:14px/1.5 'Cascadia Mono','Cascadia Code',Consolas,'Lucida Console',ui-monospace,Menlo,monospace}}\
.win{{width:min(44rem,calc(100% - 2rem));background:var(--window);border:1px solid var(--accent);\
box-shadow:0 0 0 1px rgba(0,0,0,.35),0 1.4rem 3.2rem rgba(0,0,0,.55)}}\
.bar{{display:flex;align-items:stretch;height:2rem;background:var(--titlebar);color:var(--text);\
font:12px/2rem 'Segoe UI',system-ui,-apple-system,sans-serif;user-select:none}}\
.icon{{width:2.2rem;display:grid;place-items:center;color:var(--accent);font:700 13px/1 Consolas,monospace}}\
.title{{flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}\
.btn{{width:2.9rem;display:grid;place-items:center;font:13px/1 'Segoe UI Symbol','Segoe UI',sans-serif;color:var(--text)}}\
.btn:hover{{background:rgba(255,255,255,.1)}}\
.close:hover{{background:#e81123;color:#fff}}\
.body{{padding:.7rem .9rem 1.1rem;min-height:15rem}}\
.banner{{color:var(--dim)}}\
.prompt{{color:var(--accent)}}\
.cmd{{color:var(--text)}}\
.result{{margin-top:.5rem;color:var(--result);font-weight:700}}\
.out{{color:var(--text);white-space:pre-wrap}}\
.cursor{{display:inline-block;width:.6em;height:.18em;vertical-align:baseline;background:var(--text);animation:blink 1s steps(1) infinite}}\
@keyframes blink{{50%{{opacity:0}}}}\
@media(prefers-reduced-motion:reduce){{.cursor{{animation:none}}}}\
</style>\
<main class=win>\
<div class=bar><span class=icon>&gt;_</span><span class=title>Cool Code &#8212; sign-in</span>\
<span class=btn aria-hidden=true>&#8212;</span><span class=btn aria-hidden=true>&#9744;</span><span class=\"btn close\" aria-hidden=true>&#10005;</span></div>\
<div class=body>\
<div class=banner>Cool Code sign-in helper</div>\
<div class=banner>This window can be closed once you are back in your terminal.</div>\
<div style=\"margin-top:.8rem\"><span class=prompt>C:\\cool-code&gt;</span><span class=cmd>cool-code sign-in</span></div>\
<div class=result>{mark} {headline}</div>\
{output}\
<div style=\"margin-top:.8rem\"><span class=prompt>C:\\cool-code&gt;</span><span class=cursor></span></div>\
</div></main>",
        bg = css(palette.background),
        window = css(palette.window),
        titlebar = css(darker(palette.window, 0.35)),
        text = css(palette.text),
        dim = css(palette.dim),
        accent = css(palette.accent),
        result = css(result),
        headline = escape(headline),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_success_page_is_a_terminal_window_that_says_so_and_tells_you_to_go_back() {
        let html = page(
            Outcome::Success,
            "You are signed in",
            "Return to your terminal.",
            &Palette::default(),
        );
        assert!(
            html.contains("C:\\cool-code&gt;</span><span class=cmd>cool-code sign-in"),
            "{html}"
        );
        assert!(
            html.contains("class=\"btn close\""),
            "a Windows-style close button"
        );
        assert!(
            !html.contains("border-radius"),
            "square corners like Windows 10"
        );
        assert!(html.contains("&#10003; You are signed in"));
        assert!(html.contains("Return to your terminal."));
        assert!(html.contains("Consolas"));
        assert!(
            html.contains("rgb(98,213,244)"),
            "the result uses the theme's good color"
        );
        assert!(!html.contains("<script"), "no scripts");
    }

    #[test]
    fn the_page_uses_the_palette_it_is_given() {
        let palette = Palette {
            background: [1, 2, 3],
            window: [4, 5, 6],
            text: [7, 8, 9],
            dim: [10, 11, 12],
            accent: [13, 14, 15],
            good: [16, 17, 18],
            bad: [19, 20, 21],
        };
        let ok = page(Outcome::Success, "ok", "", &palette);
        for color in [
            "rgb(1,2,3)",
            "rgb(4,5,6)",
            "rgb(7,8,9)",
            "rgb(10,11,12)",
            "rgb(13,14,15)",
            "rgb(16,17,18)",
        ] {
            assert!(ok.contains(color), "{color} missing");
        }
        assert!(
            !ok.contains("rgb(19,20,21)"),
            "the failure color is not used on success"
        );
        assert!(!ok.contains("rgb(98,213,244)"), "no default color leaks in");
    }

    #[test]
    fn a_failure_page_looks_different_and_text_is_escaped() {
        let html = page(
            Outcome::Failure,
            "Sign-in did not finish",
            "<b>access_denied</b> & more\nsecond line",
            &Palette::default(),
        );
        assert!(html.contains("&#10005; Sign-in did not finish"));
        assert!(html.contains("rgb(235,80,80)"));
        assert!(
            html.contains("&lt;b&gt;access_denied&lt;/b&gt; &amp; more"),
            "{html}"
        );
        assert!(!html.contains("<b>access_denied"), "{html}");
        assert_eq!(
            html.matches("class=out").count(),
            2,
            "one output line per line of detail"
        );
    }
}
