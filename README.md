# Harness

A coding-focused AI harness built from scratch in Rust. It is intended to be a trustworthy coding environment—not a thin chat wrapper or a fork of another harness.

Cross-platform from the beginning: Windows, macOS, and Linux are first-class targets. HarmonyOS/OpenHarmony, Haiku OS, FreeBSD, and NetBSD are planned as later, second-class targets. Platform-specific behavior (terminal capabilities, process launching, credential storage, and path conventions) belongs behind portable interfaces rather than being assumed by the core.

The native compiler toolchain is LLVM/Clang-based. GCC is not used.

> Early preview: basic non-streaming chat and an initial bounded tool loop are implemented. Streaming, dynamic workflows, and subagents are not available yet.

## Build and run

```sh
cargo run -- --help
cargo run
cargo run -- init
cargo run -- config show
cargo run -- effort
cargo run -- effort super
```

Running without a subcommand opens the interactive TUI. On first launch a short setup asks about the theme, reduced motion, local usage stats and saved sessions; stats and sessions are off unless you say yes (and can be switched in Settings → General later). With saving on, conversations are saved as sessions you can continue: `harness --resume` asks which earlier session in the current folder to resume, `harness --latest` continues the most recent one, and `--all-folders` widens either to every folder. Inside the TUI, `/resume` opens the same picker (`/resume all` lists every folder; `x` deletes a session, Tab toggles folders) and `/clear` starts a new session while keeping the old one. Sessions are plain files in `~/.coolcode/sessions/` and contain the full conversation, including tool output and any text you typed, exactly as it was before privacy redaction; delete them from the picker or by removing the files. It includes a welcome screen, prompt area, conversation view, permission/model/effort status, horizontal `/effort` slider, provider/model selection, ranked model chains, and a wider block-letter COOL CODE wordmark with a brief ice-bloom animation. Basic non-streaming chat supports OpenAI-compatible endpoints, native Anthropic Messages, and native Google Generative Language requests. All three adapters translate the harness's common tool contract; tools require workspace trust and are constrained by the permission mode. Gemini/Google and GLM/Z.ai requests require first-use acknowledgement and local reversible text redaction. Image contents are unredacted: they are sent only after separate, explicit image-content consent.

On first launch in a folder without `.coolcode/trusted`, the TUI asks whether you trust that workspace. Trust enables reading `COOL.md` and explicitly referenced `@path` files; declining leaves those reads disabled. The decision is stored in `.coolcode/trusted` and can be revoked from **Settings → Privacy**. In a trusted workspace, the model can propose line-ranged replacements and insertions, create new files without overwriting existing ones, and run shell commands. The active permission mode determines what runs automatically and what is shown for explicit approval.

The built-in harness policy prompt is compiled into the Rust application. Optional user-authored global instructions are loaded from `~/.coolcode/COOL.md`; trusted workspace `COOL.md` is supplied separately as project context. Neither file can override deterministic harness policy. The harness defines a common tool contract independently of any model; OpenAI-compatible, Anthropic, and Google adapters translate it to each API's function-calling format. Available tools include bounded repository inspection, `replace_in_file` (one-based inclusive line range plus exact expected text), `write_to_file` (insert at a one-based line; line 0 is reserved for an empty file), `create_file` (complete initial content, never overwrites), and `run_command` (PowerShell on Windows; `sh -lc` on POSIX). Edits are checked for stale file contents and shown for approval according to the active mode. The runtime enforces workspace boundaries, permission modes, approval prompts, a five-minute command timeout, and a per-turn tool-call budget (40 rounds by default; set `max_tool_rounds` in `~/.coolcode/config.toml` to change it, or reply "continue" when a turn stops at the limit).

Initial permission-mode behavior: **Plan** is read-only while the model inspects, then asks you to approve one exact plan containing the proposed snippets and commands; after approval it can perform only those listed actions. **Accept Edits** automatically applies proposed edits but asks before commands; **Accept Minimal** automatically applies edits and exact allowlisted verification commands (`cargo fmt --check`, `cargo check`, `cargo test`, `npm test`, `npm run build`, `pytest`), asking for approval otherwise; **Auto** applies small non-sensitive edits and that verification-command allowlist automatically; **Accept Everything** automatically approves edits and shell commands. The mode policy is enforced by the harness, not by model recommendations. All approved command text is shown before execution unless the active mode explicitly auto-approves it.

Slash commands and tool results are recorded as plain command output (without the assistant-answer bullet); assistant prose uses the bullet marker. The conversation returns to the bottom after a response. Use **Ctrl+Up/Down** to scroll history.

Useful repository commands include `/files`, `/read <relative-path>`, `/search <literal text>`, and `/git status`. These all require workspace trust and are read-only.

## Connect a chat model

**Workflows.** With Dynamic workflows unlocked, Super, Ultimate (and lower levels with the Workflows box ticked) give the assistant a `spawn_subagents` tool. `explore` subagents read and search in parallel; `implement` subagents can also edit and run commands, one after another, and every approval they ask for is labelled with the subagent's name. Subagents cannot start more subagents, and they see only the instructions they were given. When the assistant finishes a change, a separate reviewer subagent inspects the uncommitted changes and replies `VERDICT: PASS` or `VERDICT: ISSUES`; issues go back to the assistant for a fix round. Super allows 4 subagents at a time, 8 per turn, 12 steps each and one review; Ultimate allows 6, 20, 25 and two reviews. Subagent and reviewer requests count in `/stats`. The progress shows up as `Subagent · …` lines while it runs, and Esc cancels all of it.

**Usage warnings.** For a provider that has a limits endpoint (MultiAI has one built in), the harness re-reads your usage after each finished turn and warns you before it runs dry: a pay-as-you-go balance under 100k tokens (or under a tenth of the largest balance it has seen), and a subscription window with 20% or less left. Each warning is announced once in the conversation, and a `!! … !!` note stays in the status line while it applies. Switch it off under **Settings → General → Usage warnings**. Providers without a limits endpoint cannot be checked, and nothing is requested at launch.

**The model works with twelve tools.** Reading and searching: `list_files` (scoped by folder and glob), `read_file` (numbered lines, with `offset`/`limit`), `search_text` (literal or regex, scoped, with context), `git_status`, `git_diff`, `git_log`. Changing: `replace_text` (exact text that must match once, the preferred edit), `replace_in_file` (a line range), `write_to_file` (insert at a line), `create_file`, and `run_command`; in Plan mode, `request_plan_approval` lets you approve an exact list of actions first. The built-in system prompt is generated from this tool list and your permission mode, so it never describes a tool the model does not have.

**Effort is real, and per model.** The `/effort` picker only offers the levels the active model actually has (for example Low–Max on recent Claude models, Low–XHigh on GPT-5/6, Low/High on Gemini 3 Pro, and none on DeepSeek, Kimi, Qwen or GLM), and sends the choice to the provider in the form it understands (`reasoning_effort`, `reasoning.effort`, `output_config.effort`, or Gemini thinking settings), capped at the model's best level. If a provider rejects the parameter the request is retried once without it and the model is remembered for the rest of the run. **Super** and **Ultimate** are XHigh and Max with *workflows* (subagents) switched on; they are locked until you turn on **Settings → General → Dynamic workflows**, because workflows can use many times more tokens. With workflows unlocked, pressing ↓ on Low, Medium or High shows a Workflows checkbox for that level, and ↓ on XHigh or Max jumps to Super or Ultimate. The effort name in the status line shines when you change it and then fades smoothly back; **Settings → Appearance → Animate effort name** keeps it animated.

**Project data lives in `~/.coolcode/`, never inside your projects.** Which folders you have trusted, and whether `CLAUDE.md` / `AGENTS.md` are loaded for each, are kept in `~/.coolcode/projects.toml`, so a repository cannot trust itself by shipping a marker file (older versions kept a `.coolcode/trusted` file in each project; those files are ignored now, so you will be asked to trust each project once more). `CLAUDE.md` and `AGENTS.md` are read only if you opt in (setup wizard, or Settings → General) and only in trusted folders; `~/.claude/CLAUDE.md` is your own global file. In a project, `/claudemd` and `/agentsmd` switch each file on or off for that project (`/claudemd on`, `/agentsmd off`). **Settings → General → Reset** clears trusted folders, sessions, providers (and their saved API keys), settings, usage stats or the setup answers, individually, together, or all at once; anything that deletes data asks first.

**Settings → Appearance** picks a theme (Cool, Galaxy, Galaxy (Void), Sakura, Mint, Autumn, Retro (CRT), Synthwave), each with its own colors and animated backdrop, and controls the backdrop: *Welcome screen* (on by default), *While chatting* (off by default) and *Dim while chatting* (on by default). Everything is saved in `~/.coolcode/config.toml` and `NO_COLOR` turns the backdrop off.

In the TUI, type `/settings`, switch to **Providers**, and press `n`. Choose a preset, then set its alias, API key, and one or more model IDs. Custom API presets also ask for a base URL. A provider can be saved as a draft and completed later with `e`. In the Providers list, **Enter** selects the default provider and **Space** enables/disables it for automatic model resolution. API keys are saved to the OS credential store, not the TOML file. Use `/model <model-id>` or model-author notation such as `/model anthropic/claude-opus-5`; if multiple auto-enabled providers offer the model, the TUI asks which one to use. **Settings → Models** and the `/model` picker show models as a collapsible tree: provider, then (for aggregators such as MultiAI) the creator of each model, then the models themselves as `Name (raw-id)`, newest first. Providers that are themselves the creator, such as Google, list their models directly. Only the active provider starts unfolded, each creator shows five models with the rest behind a "more" row, and the active model is always visible. Use ↑/↓ to move, →/← to unfold or fold, Enter to fold a heading or (in `/model`) choose a model; typing in `/model` filters across everything. A model series can be named instead of a full ID: `/model fable` selects the newest listed Fable, while `/model fable-5` selects exactly version 5.0 (`fable-5.1` or `fable-5-1` selects 5.1). An unlisted model ID can be forced through the selected/default provider.

**MultiAI** is a built-in preset (`https://multiai.store/v1`). It lists its own models and reports usage, so after you save it with an API key the harness loads the model list in the background and enables the provider when it arrives. Any **Custom OpenAI-compatible API** provider can declare the same optional endpoints in its form: a *Models endpoint* (a standard `GET /models` listing) and a *Limits endpoint* (usage or balance). Each is a path relative to the base URL or a full URL, and it must be on the **same host** as the base URL and use HTTPS (HTTP only for localhost), because requests to it carry the API key; redirects are never followed. In **Settings → Providers**, `f` refreshes a provider's models (new models are added and your names are kept; non-text models are skipped; `free` and `no tools` tags appear in the model pickers) and `u` refreshes its usage, which is also loaded when you select the provider and cached for a minute. Nothing is fetched until you save a provider or open its entry.

**Signing in with ChatGPT Plus/Pro (unofficial).** When you add the **OpenAI (ChatGPT)** provider it first asks how to sign in: with an **API key** (the official, pay-per-use route) or with a **ChatGPT Plus/Pro** subscription. The subscription route opens your browser, you sign in to ChatGPT, and the page returns to the harness on `localhost:1455`; usage then counts against your subscription instead of API billing. It is **not an official or supported API**: it uses the same sign-in as OpenAI's Codex CLI, it can stop working at any time without notice, and it may conflict with OpenAI's terms of use, so use it at your own risk. Only a renewable sign-in token (and your account ID and email) is stored, in the OS credential store; short-lived access tokens stay in memory. Press **Enter** on the provider to sign in again, or delete it to sign out. The provider is listed as *ChatGPT Plus/Pro (unofficial)*, and its model list can be edited in Settings → Models.

AgentRouter is not currently included as a built-in preset because its API rejected this harness as an unauthorized client during testing. If AgentRouter authorizes the client, it can still be configured manually with **Custom OpenAI-compatible API**.

Create fallback groups in **Settings → Auto-switch models**. Add model/provider pairs in preference order, optionally enable a chain automatically when selecting one of its models, and use `/chain <id>` to start with the highest-ranked entry. **Alt+C** toggles the current chain; when a request hits a recognized usage/rate limit, the harness tries the remaining chain members in preference order. Add personal values for local redaction with `/privacy add <value>`; they are stored in the OS credential store. `/privacy clear` removes those custom values and `/privacy revoke` clears first-use acknowledgements.

The CLI also supports environment-based setup:

```sh
cargo run -- config set --provider openai --model <model-id>
# or
cargo run -- config set --provider groq --model <model-id>
```

For another OpenAI-compatible service, set its API root and the environment variable name holding its key:

```sh
cargo run -- config set --provider openai-compatible --base-url <api-root> --model <model-id> --api-key-env <ENV_VAR>
```

Then run `cargo run` and enter a prompt. API keys are read from the environment and are not saved in the config file. `HARNESS_API_KEY`, `HARNESS_MODEL`, and `HARNESS_BASE_URL` can override the configured values for a process.

For Gemini/Google or GLM/Z.ai, the first request shows a warning and requires explicit acknowledgement. Before transmission, the harness best-effort redacts common API keys, email addresses, phone-like numbers, and custom values. The reversible mapping remains in memory and matching placeholders in the reply are restored locally. This is not a guarantee of privacy. Image contents cannot be inspected or redacted locally; a separate checkbox in the warning must be enabled before image contents can be sent. The grant can be cleared under **Settings → Privacy** or with `/privacy revoke`.

Settings and interactive selectors support arrow-key navigation as well as Tab/Enter controls. Use Left and Right to switch settings tabs; the movement is directional (Left no longer advances to the next tab).

Settings are stored in `~/.coolcode/config.toml`, next to the optional global `~/.coolcode/COOL.md`. On first launch, an existing config from the previous location (the platform's user config directory under `harness/config.toml`) is copied over and the old file is kept as a backup. API credentials use environment variables or the OS credential store; they are never written to this TOML file.

## Usage stats

`/stats` opens a full-screen summary of your usage: total tokens (input and output), your favorite model, requests and turns, active days and streaks, your most active day and peak hour, your longest turn, and an activity heatmap, with a Models tab ranking each model's share and speed. Switch between all time, the last 30 days, and the last 7 days with the arrow keys. `/stats clear` deletes the history after a confirmation.

Recording is **opt-in**: on first launch a prompt asks, and the choice can be changed any time under Settings → General → Usage stats. When on, each model request appends one line to `~/.coolcode/stats.jsonl` with a timestamp, provider and model names, token counts, duration, tool-call count, and outcome. It never contains prompts or answers, and nothing is sent anywhere. Providers that do not report token usage are estimated from text length and shown with a `~`.

## Product principles

- Standalone implementation; no harness fork.
- Coding-workspace-first: repository understanding, edits, verification, and Git-aware workflows.
- Provider-neutral core, with built-in provider presets and custom compatible APIs.
- The harness—not the model—enforces permissions and resource limits.
- Plans, actions, edits, and verification should be inspectable and resumable.

See [docs/architecture.md](docs/architecture.md) for how the pieces fit together.
