# Architecture sketch

## Design constraints

This project is an original implementation. It should not depend on internal APIs or code copied from another coding harness. Provider APIs are adapters behind a provider-neutral runtime.

Windows, macOS, and Linux are first-class targets from the initial implementation. HarmonyOS/OpenHarmony (including laptop devices), Haiku OS, FreeBSD, and NetBSD are later, second-class targets. Core logic must not assume a particular shell, path separator, home directory, credential store, terminal, or process model. Put platform differences behind small adapters and test the shared behavior plus platform-specific edge cases in CI. Use Rust path/process APIs and shell-free process invocation by default; shell execution must be an explicit, policy-governed tool with platform-aware profiles rather than string concatenation into one assumed shell.

Native compilation uses LLVM/Clang tooling only; GCC is explicitly out of scope. Windows can use `clang-cl` and `lld-link` with the MSVC ABI, which still requires compatible MSVC CRT/link libraries and the Windows SDK. LLVM-MinGW can be evaluated as an LLVM-only alternative where appropriate; do not invoke GCC as a compiler.

## Proposed layers

1. **CLI/TUI** — commands, slash-command parsing, picker state, terminal rendering, cancellation.
2. **Application/session** — conversation lifecycle, persistence, settings resolution, user-visible events.
3. **Agent runtime** — bounded model turn loop, context assembly, tool-call validation, budgets, tracing.
4. **Policy engine** — deterministic authorization for reads, edits, processes, network, secrets, and workflow delegation.
5. **Tools/workspace** — repository boundaries, filesystem, search, Git, process runner, diagnostics.
6. **Provider adapters** — provider-specific transport and capability translation into normalized runtime types.
7. **Workflow engine** — persisted DAG/task graph, bounded parallel subagents, validation, resume, cost accounting.
8. **Privacy boundary** — provider/product risk classification, one-time acknowledgement state, outbound redaction, and safe local restoration.

The policy engine is authoritative. Model-generated reasoning may recommend actions but must not modify policy or self-approve. Subagents inherit the parent task's limits and cannot widen them.

The system prompt has three separately labeled sources: an immutable, versioned-in-code harness policy; optional user-authored global instructions at `~/.coolcode/COOL.md`; and project context from a trusted workspace's `COOL.md`. User instructions express the user's preferences but cannot override harness policy. Repository context and tool output are untrusted data and cannot grant permissions. The harness owns a provider-neutral tool registry; OpenAI-compatible, Anthropic, and Google adapters translate its schemas and normalized tool results into their native API formats. The agent loop includes bounded repository inspection, exact-snippet edits, and permission-gated platform shell commands. Tool proposals cannot change harness policy. Modes have deterministic rules: Plan starts read-only, then grants only the exact action list shown in an approved plan; Accept Edits auto-approves edits; Accept Minimal auto-approves edits and a literal verification-command allowlist; Auto auto-approves small non-sensitive edits and that allowlist; Accept Everything auto-approves both edits and commands. Other actions require an explicit user decision. Shell execution uses an explicit PowerShell or POSIX shell profile and is bounded by a time and output limit.

## Source layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI entry point, settings types, and config commands |
| `src/agent.rs` | Bounded agent turn loop, tool execution, and the event/approval types the UI consumes; has no terminal-UI dependencies |
| `src/policy.rs` | Deterministic permission rules (`auto_approve_*`) and the permission-mode table |
| `src/provider.rs` | Provider adapters and model fallback |
| `src/chatgpt_auth.rs` | The unofficial ChatGPT Plus/Pro sign-in: OAuth with PKCE through a local redirect server, token renewal, and the account kept in the credential store (access tokens only in memory) |
| `src/responses.rs` | Builds requests in the OpenAI Responses format, which the ChatGPT sign-in backend speaks |
| `src/context.rs` | Keeping a long conversation inside the context window: token estimates, the model-reported window, where to cut, and the summary that replaces the older messages |
| `src/tui/usage_view.rs` | `/usage`: every provider's usage (live where an endpoint exists, a dashboard pointer otherwise) |
| `src/tui/chatgpt_login.rs` | The sign-in screens: the API-key-or-subscription question, waiting for the browser, and saving the finished sign-in as a provider |
| `src/stream.rs` | Server-sent-event parsing for each adapter, stream events, and on-the-fly redaction restoring |
| `src/endpoints.rs` | Provider models and limits endpoints: same-host URL validation, tolerant model-list parsing and merging, usage summaries, and the redirect-refusing fetch |
| `src/stats.rs` | Opt-in usage recording to `~/.coolcode/stats.jsonl` (metadata only), the pure summary engine (totals, favorite model, streaks, peak hour, heatmap grid), and the fun size comparison |
| `src/workflow.rs` | Workflows: the `Completer` seam (real providers or scripted replies in tests), per-tier budgets, the subagent tool loop, the parallel-explore / sequential-implement spawner, and the reviewer with its verdict parsing |
| `src/prompt.rs` | The built-in system prompt, assembled from the live tool registry, the permission mode and the environment (working folder, platform, date) |
| `src/tools/readtools.rs` | The read-only exploration tools: glob-scoped listing, numbered ranged reads, scoped regex search with context, `git_diff` and `git_log` |
| `src/effort_support.rs` | Which effort levels each model has, how a level becomes a request parameter per API (with a remembered fallback when a provider rejects it), and the lock on the workflow tiers |
| `src/projects.rs` | Per-project choices in `~/.coolcode/projects.toml`: trusted folders and the CLAUDE.md / AGENTS.md switches; nothing is stored inside a project, so a cloned repository cannot trust itself |
| `src/session.rs` | Saved conversations (opt-in): one file per session under `~/.coolcode/sessions/` (header line plus body, written atomically) |
| `src/tools.rs` | Workspace tools, edit and create proposals |
| `src/secrets.rs` | OS credential-store access |
| `src/tui/mod.rs` | Terminal setup and the event loop |
| `src/tui/state.rs` | Application state, draft structs, and constants |
| `src/tui/commands.rs` | Prompt submission and slash-command handling |
| `src/tui/models.rs` | Model, provider, and chain resolution and activation |
| `src/tui/series.rs` | Resolves series names such as `fable` or `fable-5` to the newest matching listed model |
| `src/tui/creators.rs` | Attributes model IDs to the lab that made them, for grouping |
| `src/tui/forms.rs` | Provider and chain editing forms: state changes and key handling |
| `src/tui/context.rs` | Workspace trust and `COOL.md` / `@path` context loading |
| `src/tui/settings/` | Full-screen settings: `mod.rs` (sidebar, focus, footer) and one module per section (`general`, `appearance`, `providers`, `models`, `auto_switch`, `privacy`) plus `sync` (background model and usage fetches reported to the UI each frame) |
| `src/tui/pickers/` | Quick pickers drawn over the chat, such as the `/model` picker |
| `src/tui/widgets/` | Reusable widgets: the filterable selectable list and the collapsible provider/creator/model tree |
| `src/tui/settings/reset.rs` | Settings > General > Reset: menu, custom checklist, confirmation with counts, and the reset itself |
| `src/tui/usage_warnings.rs` | Turns a provider's limit lines into warnings (low and critical, per window or balance), announces each once, and keeps the worst one for the status line |
| `src/tui/setup.rs` | The first-run setup wizard: theme (with live preview), reduced motion, usage stats and saved sessions; only unanswered questions are asked |
| `src/tui/present.rs` | Puts each frame on the terminal: synchronized update, cursor hidden only while cells are written, nothing written for an unchanged frame |
| `src/tui/stats_view.rs` | The `/stats` full-screen view: overview, models tab, range selection, and clearing history |
| `src/tui/sessions.rs` | Saving the live conversation, resuming a saved one, and the `/resume` picker |
| `src/tui/render/` | Frame drawing: `mod.rs` (layout, input, streaming text and status line), `forms.rs`, `dialogs.rs`, `motion.rs` (text pulse and reduced-motion prompt) |
| `src/tui/effort.rs` | Effort slider rendering and animation |
| `src/tui/wordmark.rs` | Welcome wordmark and gradient |
| `src/tui/backdrop.rs` | The animated backdrop: one kind per theme (snow, stars with a nebula, plain stars, petals, bubbles, leaves, CRT noise, synthwave sun and grid), always drawn first and only into empty cells |
| `src/tui/theme.rs` | The theme table (accent, panel, prompt and screen colors, logo gradients, backdrop kind) and the per-thread current theme that `draw` sets from the settings each frame |

## Terminal experience

The default interactive launch should feel like a coding workspace, not a bare prompt loop: a centered project wordmark in ASCII art, a comfortable prompt area with subtle contrast, and a status strip showing permission mode, model, and effort separated by small dots. The UI must adapt to terminal width, support reduced/no color, and keep status visible during a session. The welcome screen shows a sparse, dim backdrop of drifting ice crystals in three depth layers; it is hidden once a conversation starts unless Settings → Appearance → While chatting is on (then it is dimmed by default, see Dim while chatting), can be turned off under Settings → Appearance, and is disabled when `NO_COLOR` is set. The look comes from the selected theme: Cool (the original, which keeps the terminal's own background), Galaxy, Galaxy (Void, near-OLED black), Sakura, Mint, Autumn, Retro (CRT) and Synthwave. Themes paint their own screen background; semantic colors (success, warning, error, permission modes) are the same in every theme. The selected brand name remains undecided.

## Effort semantics

Effort is a user-facing intent, not a provider API promise. A resolver maps the selected effort and model capabilities to supported API parameters and workflow behavior, and returns an explicit explanation for unsupported mappings.

| Level | Intended behavior | Picker highlight |
| --- | --- | --- |
| Low | Provider's low/normal-low effort | Default |
| Medium | Provider's medium effort | Default |
| High | Provider's high effort | Default |
| XHigh | Standalone provider xhigh effort; no workflows | Animated indigo/violet |
| Max | Provider's maximum normal effort | Rainbow |
| Super | xhigh + dynamically orchestrated workflows/subagents | Yellow gradient |
| Ultimate (formerly Extreme) | max + workflows/subagents; explicit cost warning | Violet-white gradient |

The picker is a horizontal segmented slider with the level labels above their matching bar segments. Only the currently selected segment is fully lit; other segments use dim, sparse stars in their own tier color, with only a very slight fade at each segment's edges. A visible but restrained glow spills into the immediate neighboring segment edges and remains localized there. The bar uses up to four rows, stacked with eighth-block glyphs, and the popup is sized to its content. Low, Medium, and High are steady fills of increasing height with subtle light only: Low breathes, Medium has a glint sweeping across, and High adds a faint shimmer to the glint. The selected segment animates continuously in time for the advanced tiers: XHigh is an aurora of two overlapping curtains in indigo and violet with teal crests, Max a rainbow equalizer whose columns bounce on smoothed noise with slowly falling peak caps, Super a traveling golden wave, and Ultimate a dark-matter void: near-black violet with slowly drifting clouds, twinkling white stars, and a softly glowing edge that breathes along the top. The tier was formerly called Extreme; settings and commands using the old name still work. Stars in unselected segments twinkle and drift slowly. The screen redraws faster only while the picker is open. TTY/terminal color support and a non-color distinction are required.

## Provider privacy and local redaction

Provider risk is not a single boolean. Classification should consider provider, product (consumer app vs API/workspace), endpoint, and user-configured privacy metadata. When a flagged model is selected in `/model`, show an explicit acknowledgement picker before accepting the selection; if a model was configured non-interactively, gate its first outbound request instead. Cancel means no request is sent. Store acknowledgement per provider/product identity and let users inspect or reset it. Avoid repeating the prompt after acknowledgement unless the product identity or warning materially changes.

For flagged providers, the user wants reversible local redaction of their name and other configured/detectable personal data. Redaction runs locally at the outbound boundary; an opaque stable token replaces a value in model-visible context, while a local-only mapping can restore it in suitable user-visible output. Never send the mapping. Code/patch restoration must be patch-aware and reviewable to avoid accidental source corruption. Redaction is a best-effort privacy aid, not a guarantee: pattern detectors miss things, values can be inferred from context, and provider-side processing still occurs.

## Initial privacy advisory examples

- **Gemini consumer app:** warn that Google's current Gemini Apps privacy notice says activity can be used to improve services, including generative AI models, when Keep Activity is on; human-reviewed chats may be retained for up to three years. With Keep Activity off, future chats are not used to train models unless feedback is sent, but they may be retained for 72 hours and used for response/safety. Work/school and API offerings can have different terms, so don't present this as a universal statement about every Google endpoint.
- **Z.ai / GLM:** warn specifically about the reported ZCode coding-client incident where workspace data was allegedly packaged and uploaded without clear consent; distinguish that client incident from hosted GLM model APIs and other Z.ai products. Use careful attribution and update or retire the warning as facts change.

Suggested acknowledgement copy: “This provider/product may handle prompts or workspace context in ways you consider sensitive. Review the details, enable local redaction, and avoid sharing secrets or confidential code. Continue once, or cancel.” The UI should summarize the specific reason and source rather than assert a blanket claim.

Initial references checked 2026-09-28:

- [Google Gemini Apps Privacy Hub](https://support.google.com/gemini/answer/13594961?hl=en) — consumer Gemini Apps data use, activity controls, and retention details.
- [Tom's Hardware report on the ZCode workspace-upload incident](https://www.tomshardware.com/tech-industry/artificial-intelligence/devs-say-chinese-ai-company-silently-uploaded-hundreds-of-megabytes-of-local-workspace-data-z-ai-the-firm-behind-the-glm-models-didnt-ask-for-user-consent-and-made-564-attempts-to-exfiltrate-313mb-archive) — third-party reporting of the coding-client incident; treat as product-specific and keep the warning sourced/qualified.

## Workflow execution

The intended workflow is durable and observable: plan -> approve according to policy -> schedule independent tasks -> collect results -> verify or attempt refutation -> retry/refine where appropriate -> synthesize -> report. It needs explicit concurrency/resource limits, checkpoints, cancellation, and resumability. Workflow progress and cost/usage should be inspectable.

## Streaming and cancellation

All adapters request streamed responses and parse server-sent events line by line from a blocking response on the worker thread; text deltas and token usage reach the UI as events, while the adapter still returns the assembled turn, including tool calls stitched together from fragments. Redaction placeholders are restored as text streams in, holding back any unfinished placeholder so partial tokens are never shown. The UI renders the growing answer with a cursor and a status line (elapsed time, reported or estimated tokens, or the running tool). Esc cancels a running turn through a shared flag: the worker stops at the next chunk, skips pending tool calls, and kills a running command; the partial answer stays in the transcript marked as interrupted and in the conversation context. Newly arrived text can pulse by word or character, and a first-run prompt offers reduced motion.

## Configuration and secrets

Settings live in `~/.coolcode/config.toml`. An existing config from the previous platform location is copied there on first launch and the old file is kept as a backup.


Settings should support user-level defaults and project-level overrides with documented precedence. Credentials must not be stored in plain project config; use environment references or the OS credential store. Effective config display must redact secret values. The TUI Providers tab stores provider profiles in TOML and API keys in Windows Credential Manager, macOS Keychain, or Linux Secret Service. Profile IDs, not API key material, link the settings record to the credential-store entry. Custom endpoint configuration must show the full destination clearly before requests are sent.

Use platform-standard config/data/cache locations through a portable directory abstraction. Implement credential storage through platform adapters (for example, Windows Credential Manager, macOS Keychain, and Linux Secret Service/keyring, with environment-variable fallback). File permissions, atomic settings/session writes, Unicode paths, symlinks, executable discovery, and terminal color/size must be handled explicitly across supported OSes.
