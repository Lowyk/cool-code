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

Running without a subcommand opens the interactive TUI. It includes a welcome screen, prompt area, conversation view, permission/model/effort status, horizontal `/effort` slider, provider/model selection, ranked model chains, and a wider block-letter COOL CODE wordmark with a brief ice-bloom animation. Basic non-streaming chat supports OpenAI-compatible endpoints, native Anthropic Messages, and native Google Generative Language requests. All three adapters translate the harness's common tool contract; tools require workspace trust and are constrained by the permission mode. Gemini/Google and GLM/Z.ai requests require first-use acknowledgement and local reversible text redaction. Image contents are unredacted: they are sent only after separate, explicit image-content consent.

On first launch in a folder without `.coolcode/trusted`, the TUI asks whether you trust that workspace. Trust enables reading `COOL.md` and explicitly referenced `@path` files; declining leaves those reads disabled. The decision is stored in `.coolcode/trusted` and can be revoked from **Settings → Privacy**. In a trusted workspace, the model can propose line-ranged replacements and insertions, create new files without overwriting existing ones, and run shell commands. The active permission mode determines what runs automatically and what is shown for explicit approval.

The built-in harness policy prompt is compiled into the Rust application. Optional user-authored global instructions are loaded from `~/.coolcode/COOL.md`; trusted workspace `COOL.md` is supplied separately as project context. Neither file can override deterministic harness policy. The harness defines a common tool contract independently of any model; OpenAI-compatible, Anthropic, and Google adapters translate it to each API's function-calling format. Available tools include bounded repository inspection, `replace_in_file` (one-based inclusive line range plus exact expected text), `write_to_file` (insert at a one-based line; line 0 is reserved for an empty file), `create_file` (complete initial content, never overwrites), and `run_command` (PowerShell on Windows; `sh -lc` on POSIX). Edits are checked for stale file contents and shown for approval according to the active mode. The runtime enforces workspace boundaries, permission modes, approval prompts, a five-minute command timeout, and a per-turn tool-call budget (40 rounds by default; set `max_tool_rounds` in `~/.coolcode/config.toml` to change it, or reply "continue" when a turn stops at the limit).

Initial permission-mode behavior: **Plan** is read-only while the model inspects, then asks you to approve one exact plan containing the proposed snippets and commands; after approval it can perform only those listed actions. **Accept Edits** automatically applies proposed edits but asks before commands; **Accept Minimal** automatically applies edits and exact allowlisted verification commands (`cargo fmt --check`, `cargo check`, `cargo test`, `npm test`, `npm run build`, `pytest`), asking for approval otherwise; **Auto** applies small non-sensitive edits and that verification-command allowlist automatically; **Accept Everything** automatically approves edits and shell commands. The mode policy is enforced by the harness, not by model recommendations. All approved command text is shown before execution unless the active mode explicitly auto-approves it.

Slash commands and tool results are recorded as plain command output (without the assistant-answer bullet); assistant prose uses the bullet marker. The conversation returns to the bottom after a response. Use **Ctrl+Up/Down** to scroll history.

Useful repository commands include `/files`, `/read <relative-path>`, `/search <literal text>`, and `/git status`. These all require workspace trust and are read-only.

## Connect a chat model

In the TUI, type `/settings`, switch to **Providers**, and press `n`. Choose a preset, then set its alias, API key, and one or more model IDs. Custom API presets also ask for a base URL. A provider can be saved as a draft and completed later with `e`. In the Providers list, **Enter** selects the default provider and **Space** enables/disables it for automatic model resolution. API keys are saved to the OS credential store, not the TOML file. Use `/model <model-id>` or model-author notation such as `/model anthropic/claude-opus-5`; if multiple auto-enabled providers offer the model, the TUI asks which one to use. An unlisted model ID can be forced through the selected/default provider.

**MultiAI** is a built-in preset (`https://multiai.store/v1`). It lists its own models and reports usage, so after you save it with an API key the harness loads the model list in the background and enables the provider when it arrives. Any **Custom OpenAI-compatible API** provider can declare the same optional endpoints in its form: a *Models endpoint* (a standard `GET /models` listing) and a *Limits endpoint* (usage or balance). Each is a path relative to the base URL or a full URL, and it must be on the **same host** as the base URL and use HTTPS (HTTP only for localhost), because requests to it carry the API key; redirects are never followed. In **Settings → Providers**, `f` refreshes a provider's models (new models are added and your names are kept; non-text models are skipped; `free` and `no tools` tags appear in the model pickers) and `u` refreshes its usage, which is also loaded when you select the provider and cached for a minute. Nothing is fetched until you save a provider or open its entry.

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

## Product principles

- Standalone implementation; no harness fork.
- Coding-workspace-first: repository understanding, edits, verification, and Git-aware workflows.
- Provider-neutral core, with built-in provider presets and custom compatible APIs.
- The harness—not the model—enforces permissions and resource limits.
- Plans, actions, edits, and verification should be inspectable and resumable.

See [ROADMAP.md](ROADMAP.md) and [docs/architecture.md](docs/architecture.md).
