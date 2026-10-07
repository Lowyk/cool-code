# Cool Code

**A temperature-conscious coding harness for your terminal, written from scratch in Rust.**

Cool Code is a terminal UI where an AI model reads your repository, proposes edits, runs commands and checks its own work, while the harness (not the model) decides what is allowed to happen. It works with the providers you already use, keeps your keys in the OS credential store, and tells you what it can and cannot see.

> **Status: early preview.** It is a real, daily-usable harness, but it is young: expect rough edges, and read the [unofficial features](#unofficial-sign-in) section before relying on the ChatGPT sign-in.

## Highlights

- **Many providers, one place.** OpenAI, Anthropic, Google, OpenRouter, MultiAI, Kimi, Z.ai, DeepSeek, Mistral, xAI, Groq, Ollama, any OpenAI- or Anthropic-compatible API, and an unofficial ChatGPT Plus/Pro sign-in. Models are listed from the provider, not hard-coded.
- **Effort that is honest.** The effort picker shows only the levels the active model really has, sends them in each provider's own form, and never shows a tier the model cannot reach.
- **Workflows.** Super and Ultimate (and any level with the box ticked) let the model split work across subagents, then have the result reviewed. Locked by default because they can use many times more tokens.
- **Permissions enforced by the harness.** Five modes from read-only *Plan* to *Accept Everything*, with every command shown before it runs unless the mode says otherwise.
- **Know what you are spending.** `/usage` shows every provider's remaining usage, and running-low warnings appear before a balance or limit runs out.
- **Private by default.** Sessions and usage stats are opt-in, project data lives outside your repositories, and outbound text can be redacted for providers that need it.
- **Looks good.** Eight themes, each with an animated backdrop, and a synchronized renderer with no flicker.

## Install

You need a recent stable [Rust toolchain](https://rustup.rs) (the 2024 edition, so Rust 1.88 or newer). Windows, macOS and Linux are all tested in CI.

```sh
git clone https://github.com/Lowyk/cool-code
cd cool-code
cargo build --release
./target/release/harness        # harness.exe on Windows
```

On Linux the credential store needs the Secret Service libraries (`libdbus-1-dev` and `pkg-config` to build). The compiler toolchain is LLVM/Clang-based; GCC is not used.

## Quick start

1. Run `harness`. A short setup asks for a theme, reduced motion, and whether to keep usage stats, saved sessions, and your `CLAUDE.md` / `AGENTS.md` files. Everything privacy-related is off unless you say yes.
2. Type `/settings`, open **Providers**, press `n`, and pick a preset. Enter an API key (or sign in, for ChatGPT). Keys go to the OS credential store, never to a file.
3. Choose a model with `/model`, set an effort with `/effort`, and start typing.
4. When asked, decide whether to **trust the folder**. Trust is what lets the model read your files and propose edits.

## Providers

| Preset | Sign-in | Models | Usage in `/usage` |
| --- | --- | --- | --- |
| OpenAI (ChatGPT) | API key, or ChatGPT Plus/Pro (unofficial) | you list them (API key); from your account (sign-in) | dashboard (API key); 5-hour and weekly windows (sign-in) |
| Anthropic (Claude) | API key | you list them | dashboard |
| Google (Gemini) | API key | starter list | dashboard |
| OpenRouter | API key | you list them | credits |
| MultiAI | API key | from the API | balance and limits |
| Kimi Code (membership) | membership API key | documented starter model | dashboard |
| Moonshot (Kimi API) | API key | from the API | dashboard |
| Z.ai GLM Coding Plan | plan API key | documented starter model | dashboard |
| DeepSeek | API key | from the API | balance |
| Mistral, xAI (Grok) | API key | from the API | dashboard |
| Ollama (local) | none | your installed models | nothing to track |
| Custom OpenAI / Anthropic compatible | API key | you list them, or a models endpoint | optional limits endpoint |

Notes:

- Press `f` in Settings → Providers to reload a provider's models and `u` to reload its usage. Custom providers can declare a *Models endpoint* and a *Limits endpoint*; both must be on the same host as the base URL.
- `/model <id>` picks a model; `/model fable` picks the newest listed model of a series. Both only consider providers with automatic switching on, or models in a preference chain. **`/forcemodel <id>`** sets any model on your default provider exactly as typed, for providers that cannot list their models.
- Kimi Code and the Z.ai plan are meant for coding tools like this one. Use their own endpoints (the presets do) and do not disguise the client.
- Gemini/Google and GLM/Z.ai requests ask for acknowledgement first and redact obvious secrets before sending (see [Privacy](#privacy-and-data)).
- Groq has no preset yet; configure it from the command line with `harness config set --provider groq --model <id>` and `GROQ_API_KEY`.
- Fallback **chains** (Settings → Auto-switch models, `/chain <id>`, `Alt+C`) move to the next model when one hits a rate limit.

### Unofficial sign-in

**ChatGPT Plus/Pro.** Adding the OpenAI provider asks whether to use an API key or your ChatGPT subscription. The subscription route opens your browser, you sign in to ChatGPT, and the page returns to Cool Code on `localhost:1455`. Usage then counts against your subscription instead of API billing.

This is **not an official or supported API.** It uses the same sign-in as OpenAI's Codex CLI, can stop working at any time, and may conflict with OpenAI's terms of use. Use it at your own risk. Only a renewable sign-in token, your account ID and your email are stored (in the OS credential store). The provider appears as *ChatGPT Plus/Pro (unofficial)*, and its models and usage come from your account. Newer models are only listed to recent Codex versions; if the list looks stale, set `COOLCODE_CODEX_CLIENT_VERSION` to a current Codex CLI version.

**Why there is no Google or Claude sign-in.** Google and Anthropic prohibit using a subscription login (Antigravity, Gemini CLI, Claude Pro/Max) in third-party tools and have suspended accounts for it. Use an API key, or an aggregator such as OpenRouter or MultiAI, instead.

## Working with the model

### Permission modes

| Mode | Behavior |
| --- | --- |
| **Plan** | Read-only while the model inspects, then you approve one exact list of actions. After approval it can do only those. |
| **Accept Edits** | Applies edits automatically, asks before commands. |
| **Accept Minimal** | Applies edits and exact allowlisted checks (`cargo fmt --check`, `cargo check`, `cargo test`, `npm test`, `npm run build`, `pytest`); asks for anything else. |
| **Auto** | Applies small, non-sensitive edits and the same allowlisted checks. |
| **Accept Everything** | Approves edits and shell commands automatically. |

The mode is enforced by the harness, not by what the model recommends. Edits are checked for stale file contents, workspace boundaries are enforced, commands time out after five minutes, and each turn has a tool-call budget (40 rounds by default; set `max_tool_rounds` in `~/.coolcode/config.toml`, or say "continue").

### Tools

The model has twelve tools. **Reading:** `list_files`, `read_file`, `search_text` (literal or regex), `git_status`, `git_diff`, `git_log`. **Changing:** `replace_text` (preferred), `replace_in_file`, `write_to_file`, `create_file` (never overwrites), `run_command` (PowerShell on Windows, `sh -lc` elsewhere). **Planning:** `request_plan_approval` in Plan mode. The system prompt is generated from this list and your mode, so it never describes a tool the model does not have.

### Effort and workflows

`/effort` offers the levels the active model has (for example Low to Max on recent Claude and GPT-5.6/GPT-6 models, Low to XHigh on GPT-5.4 and 5.5, Low and High on Gemini 3 Pro, none on models without a control) and sends the choice in the provider's own form. If a provider rejects the parameter, the request is retried without it and remembered.

**Super** is XHigh with *workflows*, and **Ultimate** is Max with workflows, so a model without XHigh or Max does not show them. They stay locked until you turn on **Settings → General → Dynamic workflows**. With workflows unlocked, ↓ on Low, Medium or High shows a Workflows checkbox for that level, and ↓ on XHigh or Max jumps to its tier.

With workflows on, the model gets a `spawn_subagents` tool. `explore` subagents read and search in parallel; `implement` subagents can also edit and run commands, one at a time, and every approval they ask for names the subagent. Subagents cannot start more subagents. When a change is finished, a separate reviewer inspects the uncommitted changes and answers `VERDICT: PASS` or `VERDICT: ISSUES`; issues go back for a fix round. Super allows 4 subagents at a time, 8 per turn, 12 steps each and one review; Ultimate allows 6, 20, 25 and two. Esc cancels everything.

### Long conversations

The status line shows how full the model's context is (`ctx 42k/200k`, yellow above 80% and red above 95%; without a reported window just the size). Before a request would fill the window, the older conversation is replaced by a short briefing the model writes, and the recent part stays word for word; if a provider says a request was too large, the same thing happens and the request is retried. `/compact` does it on demand, and **Settings → General → Auto-compact** turns the automatic version off. The window comes from what the provider reports for the model, so a provider that does not report one is only condensed when it complains. The summary is an estimate-driven approximation: sizes are counted at about four characters a token.

### Project instructions

`COOL.md` (project guidance), `CLAUDE.md` and `AGENTS.md` can be loaded as context, only in trusted folders. `CLAUDE.md` and `AGENTS.md` are opt-in; `/claudemd` and `/agentsmd` switch each on or off for the current project, and `~/.claude/CLAUDE.md` is your own global file. A global `~/.coolcode/COOL.md` works too. None of these can override the harness's policy.

## Commands

| Command | Does |
| --- | --- |
| `/help` | List commands |
| `/settings`, `/provider`, `/chain [id]` | Open settings, providers, or switch a model chain |
| `/model [id]`, `/forcemodel <id>` | Choose a model, or force one on the default provider |
| `/effort [level]`, `/mode [name]` | Effort picker, permission mode |
| `/usage`, `/stats [clear]` | Provider usage, your own usage history |
| `/compact` | Condense the older conversation into a summary now |
| `/files`, `/read <path>`, `/search <text>`, `/git status` | Read-only repository helpers (trusted folders) |
| `/init` | Create a starter `COOL.md` |
| `/claudemd`, `/agentsmd` | Toggle those files for this project |
| `/privacy [add\|clear\|revoke]` | Local redaction values and acknowledgements |
| `/resume [all]`, `/clear`, `/quit` | Sessions and exit |

Attach workspace files with `@path`. **Ctrl+Up/Down** scrolls the conversation. The command line also has `harness config`, `harness effort`, `harness init`, `harness --resume`, `harness --latest` and `--all-folders`.

## Privacy and data

- **Everything lives in `~/.coolcode/`**, never inside your projects: `config.toml`, `projects.toml` (which folders you trust, so a repository cannot trust itself), optional `sessions/`, and optional `stats.jsonl`. API keys and sign-in tokens use the OS credential store.
- **Sessions are opt-in.** They are plain files holding the full conversation exactly as typed, before redaction. Resume with `/resume` or `harness --resume`.
- **Usage stats are opt-in** and record only timestamps, provider and model names, token counts, durations and outcomes. Never prompts or answers, and nothing is sent anywhere. `/stats` shows a summary and a heatmap.
- **Redaction.** For providers that require it, common API keys, emails, phone-like numbers and your custom values are replaced by placeholders before sending, and restored locally in the reply. This is best-effort, not a guarantee, and images cannot be inspected, so they need their own consent.
- **Reset.** Settings → General → Reset clears trusted folders, sessions, providers (and their keys), settings or stats, individually or all at once, always after asking.

## Appearance

Settings → Appearance has eight themes (Cool, Galaxy, Galaxy (Void), Sakura, Mint, Autumn, Retro (CRT), Synthwave), each with a colour set and an animated backdrop, plus options for the backdrop on the welcome screen and while chatting, and for animating the effort name. `NO_COLOR` turns the backdrop off. The browser page shown after a sign-in matches your theme.

## Principles

- A standalone implementation, not a fork of another harness.
- Coding-workspace-first: repository understanding, edits, verification, Git awareness.
- A provider-neutral core with presets and custom compatible APIs.
- The harness enforces permissions and limits, not the model.
- Plans, actions, edits and verification stay inspectable.

## Contributing and layout

See [docs/architecture.md](docs/architecture.md) for how the pieces fit together. Before sending a change, run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test`; CI runs the same on Linux, Windows and macOS.

## License

[MIT](LICENSE).
