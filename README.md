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
- **Looks good.** Answers are drawn as Markdown (headings, code blocks, lists, tables), there are eight themes with animated backdrops and a light mode, and a synchronized renderer with no flicker.

## Install

**Windows.** Download `coolcode-v0.1.0-windows-x64-setup.exe` from the [releases page](https://github.com/Lowyk/cool-code/releases) and run it. It installs for your user (no administrator prompt) and can add `coolcode` to your PATH; open a new terminal afterwards and type `coolcode`. Windows may show a SmartScreen warning because the installer is not code-signed yet: choose *More info*, then *Run anyway*. Uninstall it from Settings → Apps.

**macOS and Linux.**

```sh
curl -fsSL https://raw.githubusercontent.com/Lowyk/cool-code/master/install.sh | sh
```

The script picks the right build for your computer, checks it against its checksum, and puts `coolcode` in `~/.local/bin` (change it with `COOLCODE_INSTALL_DIR`; pick a version with `COOLCODE_VERSION=v0.1.0`). It tells you if that folder is not on your `PATH`.

**By hand.** Each release also has an archive for every system (Windows, macOS on Apple silicon and Intel, Linux) with a `.sha256` checksum file. Unpack one and put `coolcode` somewhere on your `PATH`.

**From source.** You need a recent stable [Rust toolchain](https://rustup.rs) (the 2024 edition, so Rust 1.88 or newer). Windows, macOS and Linux are all tested in CI.

```sh
cargo install --git https://github.com/Lowyk/cool-code   # installs `coolcode`
# or, to work on it:
git clone https://github.com/Lowyk/cool-code
cd cool-code
cargo build --release
./target/release/coolcode        # coolcode.exe on Windows
```

On Linux the credential store needs the Secret Service libraries (`libdbus-1-dev` and `pkg-config` to build). The compiler toolchain is LLVM/Clang-based; GCC is not used.

## Quick start

1. Run `coolcode`. A short setup asks for a theme, reduced motion, and whether to keep usage stats, saved sessions, and your `CLAUDE.md` / `AGENTS.md` files. Everything privacy-related is off unless you say yes.
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
| Mistral, xAI (Grok), Groq | API key | from the API | dashboard |
| Ollama (local) | none | your installed models | nothing to track |
| Custom OpenAI / Anthropic compatible | API key | you list them, or a models endpoint | optional limits endpoint |

Notes:

- Press `f` in Settings → Providers to reload a provider's models and `u` to reload its usage. Custom providers can declare a *Models endpoint* and a *Limits endpoint*; both must be on the same host as the base URL.
- `/model <id>` picks a model; `/model fable` picks the newest listed model of a series. Both only consider providers with automatic switching on, or models in a preference chain. **`/forcemodel <id>`** sets any model on your default provider exactly as typed, for providers that cannot list their models.
- Kimi Code and the Z.ai plan are meant for coding tools like this one. Use their own endpoints (the presets do) and do not disguise the client.
- Gemini/Google and GLM/Z.ai requests ask for acknowledgement first and redact obvious secrets before sending (see [Privacy](#privacy-and-data)).
- Anthropic's own API gets prompt caching: the system prompt and the conversation so far are marked for reuse, so long sessions pay the cheaper cached price. Compatible servers are not sent the marker. Rate limits and server errors are retried (up to three times, waiting as the provider asks) before a request is reported as failed; a quota or billing limit is not retried and moves on to your fallback chain.
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
| **Manual** | Asks before every edit, new file and command. Only reading and searching are automatic. |
| **Auto** | Greyed out until you choose guard models in **Settings → Auto Mode**. Then every command, edit and new file is shown to those models, which answer yes or no; a no asks you and shows why (see below). |
| **Accept Everything** | Approves edits and shell commands automatically. |

**Setting up Auto mode.** Open **Settings → Auto Mode** and pick the models that should act as guards. They come from providers you already added, with the small, fast families listed first as recommendations: Luna, Haiku, Flash, Flash-Lite and Safety models (for example `gpt-oss-safeguard`) can judge an action. Llama Prompt Guard models are classifiers: they only scan for injected instructions and can flag an action, never approve one, so you need at least one judge as well. Until a judge is chosen, picking Auto shows *Go to Settings > Auto Mode to activate*, and `coolcode run --mode auto` refuses to start.

**How Auto mode decides.** Fixed rules come first and cannot be overruled: commands that delete trees, force-push, run downloaded code, use `sudo`, touch credentials or system settings, or send data over the network, and files that look like secrets (`.env`, keys, anything named like credentials), are never shown to a guard and always go to you. For the rest, the first guard that can answer sees the action as quoted data, with your request for context, and replies `yes` or `no` in strict JSON. A yes runs the action; a no asks you, with the guard's reason, and if you approve it the action runs as in any other mode. Guards are asked in the order you set (move them with `u`/`d`), so a later one covers for an earlier one that is out of usage, offline or gave an unreadable reply. If none can answer, the action does not run and the model is told *Auto Mode isn't currently available. Ask the user to switch your mode to Plan, Accept Minimal, Accept Edits, or Manual*. Removing the last judge, or deleting its provider, switches Auto off to Manual. Guards run at a low effort on their own provider and never fall over to your chat chain. Every check is one more request to that provider, and the command or edit it looks at is sent there; a provider that needs the one-time privacy acknowledgement must have had it (by sending it a message once) or its guard counts as unable to answer. Only Auto mode makes these requests.

The mode is enforced by the harness, not by what the model recommends. Edits are checked for stale file contents, workspace boundaries are enforced, commands time out after five minutes, and each turn has a tool-call budget (40 rounds by default; set `max_tool_rounds` in `~/.coolcode/config.toml`, or say "continue").

### Tools

The model has twelve tools. **Reading:** `list_files`, `read_file`, `search_text` (literal or regex), `git_status`, `git_diff`, `git_log`. **Changing:** `replace_text` (preferred), `replace_in_file`, `write_to_file`, `create_file` (never overwrites), `run_command` (PowerShell on Windows, `sh -lc` elsewhere). **Planning:** `request_plan_approval` in Plan mode. The system prompt is generated from this list and your mode, so it never describes a tool the model does not have.

### Effort and workflows

`/effort` offers the levels the active model has (for example Low to Max on recent Claude and GPT-5.6/GPT-6 models, Low to XHigh on GPT-5.4 and 5.5, Low and High on Gemini 3 Pro, none on models without a control) and sends the choice in the provider's own form. If a provider rejects the parameter, the request is retried without it and remembered.

**Super** is XHigh with *workflows*, and **Ultimate** is Max with workflows, so a model without XHigh or Max does not show them. They stay locked until you turn on **Settings → General → Dynamic workflows**. With workflows unlocked, ↓ on Low, Medium or High shows a Workflows checkbox for that level, and ↓ on XHigh or Max jumps to its tier.

With workflows on, the model gets a `spawn_subagents` tool. `explore` subagents read and search in parallel; `implement` subagents can also edit and run commands, one at a time, and every approval they ask for names the subagent. Subagents cannot start more subagents. When a change is finished, a separate reviewer inspects the uncommitted changes and answers `VERDICT: PASS` or `VERDICT: ISSUES`; issues go back for a fix round. Super allows 4 subagents at a time, 8 per turn, 12 steps each and one review; Ultimate allows 6, 20, 25 and two. Esc cancels everything.

### Placeholder images

If you add an image API, the model can make placeholder pictures while it builds (a hero image, an icon, sample art). It is **off until you set it up**: **Settings → General → Image generation** asks for an OpenAI-style images API address (`https://api.openai.com/v1` and `gpt-image-1` are offered), a model and an API key, which is kept in the OS credential store. Only then does the model get a `generate_image` tool, and nothing is offered to subagents. Every image costs money on that API, so each one asks for your approval (the prompt, the size, the model and where it will be saved) in every mode except Accept Everything; in Plan mode it has to be in the approved plan. Images are saved as new `.png`, `.jpg`, `.jpeg` or `.webp` files inside the project, folders are created, an existing file is never replaced, and `/undo` removes a generated image unless you have changed it since. **Turn off** in the same screen deletes the key. The API address must be HTTPS (or on this computer), and Reset → Providers removes the image setup too.

### Referencing files with @

Type `@` to attach a file to your message. A list of the project's files appears as you type (best matches first, build output and anything that looks like a secret left out); **↑/↓** choose, **Tab** or **Enter** insert, **Esc** closes it. A path with a separator browses that folder (`@src/`), and names with spaces are quoted (`@"my notes.txt"`). Images attach too. References inside the project work in a trusted folder as before, and `@./x` or `@src/../x` are fine as long as they stay inside.

Files **outside** the project (`@../x`, `@~/x`, a full path, or a link that leads out) are refused until you turn on **Settings → Privacy → Outside files**. Even then, every such file is shown to you and has to be confirmed with **y** before it is read, one at a time, and answering **n** sends nothing and gives you your text back. A path that looks like it may hold secrets (`.env`, keys, `.ssh`, anything named like credentials) is flagged in red. There is also a hidden second option that stops the confirmations for ordinary files (secret-looking paths always still ask); it appears if you switch the Outside files setting on and off six times in quick succession.

### Undo

`/undo` takes back the file changes the model made in its last turn, one turn at a time (up to 20 this session). It restores edited files and removes files the turn created, but only if a file is still exactly as the turn left it: anything you or a command changed since is left alone and reported. The model is told about the undo with your next message. Only changes made through the model's edit tools are tracked, not what `run_command` did, and the history is kept for the current run only.

### Long conversations

The status line shows how full the model's context is (`ctx 42k/200k`, yellow above 80% and red above 95%; without a reported window just the size). Before a request would fill the window, the older conversation is replaced by a short briefing the model writes, and the recent part stays word for word; if a provider says a request was too large, the same thing happens and the request is retried. `/compact` does it on demand, and **Settings → General → Auto-compact** turns the automatic version off. The window comes from what the provider reports for the model, so a provider that does not report one is only condensed when it complains. The summary is an estimate-driven approximation: sizes are counted at about four characters a token.

### Scripts and CI

`coolcode run "what to do"` runs one turn without the interface (or pipe the prompt in: `git diff | coolcode run "review this"`). The answer goes to standard output and progress to standard error; `--json` prints one JSON object per line instead (`tool`, `file`, `declined`, `note` and a final `result`). It uses your saved provider and settings, with `--model`, `--mode` and `--effort` to override them for one run. The folder must already be trusted for the model to have tools (or pass `--trust` for that run only; nothing is saved). There is nobody to ask for approval, so anything the permission mode would ask about is declined and reported: pick a mode that fits the job, for example `--mode accept-edits`. Super and Ultimate stay locked unless Dynamic workflows is on, and a provider that needs a one-time privacy acknowledgement refuses until you have given it in the interface. Headless runs are not saved as sessions.

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
| `/undo` | Take back the file changes from the model's last turn |
| `/files`, `/read <path>`, `/search <text>`, `/git status` | Read-only repository helpers (trusted folders) |
| `/init` | Create a starter `COOL.md` |
| `/claudemd`, `/agentsmd` | Toggle those files for this project |
| `/privacy [add\|clear\|revoke]` | Local redaction values and acknowledgements |
| `/resume [all]`, `/clear`, `/quit` | Sessions and exit |

Attach files with `@path` (see above). **Shift+Enter** or **Alt+Enter** starts a new line, and a pasted block of lines stays in the prompt until you press Enter. **Ctrl+Up/Down** scrolls the conversation. The command line also has `coolcode run`, `coolcode config`, `coolcode effort`, `coolcode init`, `coolcode --resume`, `coolcode --latest` and `--all-folders`.

## Privacy and data

- **Everything lives in `~/.coolcode/`**, never inside your projects: `config.toml`, `projects.toml` (which folders you trust, so a repository cannot trust itself), optional `sessions/`, and optional `stats.jsonl`. API keys and sign-in tokens use the OS credential store.
- **Sessions are opt-in.** They are plain files holding the full conversation exactly as typed, before redaction. Resume with `/resume` or `coolcode --resume`.
- **Usage stats are opt-in** and record only timestamps, provider and model names, token counts, durations and outcomes. Never prompts or answers, and nothing is sent anywhere. `/stats` shows a summary and a heatmap.
- **Redaction.** For providers that require it, common API keys, emails, phone-like numbers and your custom values are replaced by placeholders before sending, and restored locally in the reply. This is best-effort, not a guarantee, and images cannot be inspected, so they need their own consent.
- **Reset.** Settings → General → Reset clears trusted folders, sessions, providers (and their keys), settings or stats, individually or all at once, always after asking.

## Appearance

Settings → Appearance has eight themes (Cool, Galaxy, Galaxy (Void), Sakura, Mint, Autumn, Retro (CRT), Synthwave), each with a colour set and an animated backdrop, plus a light mode, options for the backdrop on the welcome screen and while chatting, and for animating the effort name. Next to the options is a live preview of the highlighted one: a theme is shown in its own colours with its backdrop before you pick it, and the effort names under "Animate effort name" move only while it is on. Light mode works with every theme: it keeps each theme's colours but draws them light. `NO_COLOR` turns the backdrop off. The browser page shown after a sign-in looks like a Windows 10 console window in your theme's colours.

## Principles

- A standalone implementation, not a fork of another harness.
- Coding-workspace-first: repository understanding, edits, verification, Git awareness.
- A provider-neutral core with presets and custom compatible APIs.
- The harness enforces permissions and limits, not the model.
- Plans, actions, edits and verification stay inspectable.

## Contributing and layout

See [CONTRIBUTING.md](CONTRIBUTING.md), [docs/architecture.md](docs/architecture.md) for how the pieces fit together, [CHANGELOG.md](CHANGELOG.md) for what changed, and [SECURITY.md](SECURITY.md) for reporting a vulnerability. Before sending a change, run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test`; CI runs the same on Linux, Windows and macOS.

## License

[MIT](LICENSE).
