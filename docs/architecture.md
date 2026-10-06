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
| `src/tools.rs` | Workspace tools, edit and create proposals |
| `src/secrets.rs` | OS credential-store access |
| `src/tui/mod.rs` | Terminal setup and the event loop |
| `src/tui/state.rs` | Application state, draft structs, and constants |
| `src/tui/commands.rs` | Prompt submission and slash-command handling |
| `src/tui/models.rs` | Model, provider, and chain resolution and activation |
| `src/tui/forms.rs` | Provider and chain editing forms and settings key handling |
| `src/tui/context.rs` | Workspace trust and `COOL.md` / `@path` context loading |
| `src/tui/render/` | Frame drawing: `mod.rs` (layout and input), `settings.rs`, `dialogs.rs` |
| `src/tui/effort.rs` | Effort slider rendering and animation |
| `src/tui/wordmark.rs` | Welcome wordmark and gradient |

## Terminal experience

The default interactive launch should feel like a coding workspace, not a bare prompt loop: a centered project wordmark in ASCII art, a comfortable prompt area with subtle contrast, and a status strip showing permission mode, model, and effort separated by small dots. The UI must adapt to terminal width, support reduced/no color, and keep status visible during a session. The selected brand name remains undecided.

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
| Extreme | max + workflows/subagents; explicit cost warning | Red gradient |

The picker is a horizontal segmented slider with the level labels above their matching bar segments. Only the currently selected segment is fully lit; other segments use dim, sparse stars in their own tier color, with only a very slight fade at each segment's edges. A visible but restrained glow spills into the immediate neighboring segment edges and remains localized there. The selected label and its bar segment animate for Max, XHigh, Super, and Extreme: Max has descending rainbow bar heights, XHigh uses the same profile at a shorter height, Super alternates tall/short bars, and Extreme flickers like fire. TTY/terminal color support and a non-color distinction are required.

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

## Configuration and secrets

Settings should support user-level defaults and project-level overrides with documented precedence. Credentials must not be stored in plain project config; use environment references or the OS credential store. Effective config display must redact secret values. The TUI Providers tab stores provider profiles in TOML and API keys in Windows Credential Manager, macOS Keychain, or Linux Secret Service. Profile IDs, not API key material, link the settings record to the credential-store entry. Custom endpoint configuration must show the full destination clearly before requests are sent.

Use platform-standard config/data/cache locations through a portable directory abstraction. Implement credential storage through platform adapters (for example, Windows Credential Manager, macOS Keychain, and Linux Secret Service/keyring, with environment-variable fallback). File permissions, atomic settings/session writes, Unicode paths, symlinks, executable discovery, and terminal color/size must be handled explicitly across supported OSes.
