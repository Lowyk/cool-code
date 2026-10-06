# Roadmap

This is a staged plan, not a claim that planned features already exist.

## M0 — Product contract and foundations

- [x] Start a standalone Rust CLI project.
- [x] Capture the initial product direction and effort levels.
- [x] Specify initial permission semantics for edits/processes; settings precedence and session lifecycle remain.
- [ ] Add CLI and config tests; establish formatting, lint, CI, and release conventions.
- [ ] Define Windows/macOS/Linux as first-class targets and add cross-platform CI from the first executable milestone.
- [ ] Keep HarmonyOS/OpenHarmony, Haiku OS, FreeBSD, and NetBSD as second-class targets for a later portability milestone.
- [ ] Use LLVM/Clang toolchains only; do not use GCC. Windows MSVC-target builds may require Microsoft's CRT/link libraries in addition to LLVM tools and the Windows SDK.

## M1 — Useful local shell

- [x] Initial interactive welcome screen: ASCII wordmark, prompt box, and permission/model/effort status row.
- [ ] Polish responsive layout, terminal capability fallback, accessibility, and keyboard affordances.
- [ ] Interactive terminal experience with streaming output and cancellation; command transcript and core slash commands are implemented.
- [x] `/settings` General/Providers tabs, add provider form, and OS credential-store API key save/load.
- [x] Horizontal `/effort [X]` segmented slider with separate XHigh, explicit effort/workflow mappings, animated highlights, and dim star-filled unselected tiers.
- [ ] Provider/model settings UI and capability reporting; secrets via OS keychain or environment.
- [ ] Session create/resume/list/inspect/export; durable, privacy-conscious local storage.

## M2 — Provider runtime

- [x] Initial normalized function-call model with OpenAI-compatible, Anthropic, and Google API encodings.
- [x] Initial non-streaming OpenAI-compatible chat adapter with OpenAI and Groq presets.
- [x] Provider profile editing, multiple profiles, default-provider selection, and explicit auto-switch activation.
- [x] Ranked model chains, model-author references, and usage-limit fallback with an opt-in active chain.
- [x] Native Anthropic Messages API adapter and request normalization for text/image inputs.
- [x] Native Google Generative Language adapter with first-use acknowledgement, reversible text redaction, and native function-calling translation.
- [ ] Custom OpenAI-compatible and Anthropic-compatible endpoints.
- [ ] Streaming, retries, timeout/rate-limit handling, cancellation, and normalized errors.
- [ ] Capability-aware effort mapping. Unsupported effort levels must be reported, never silently claimed.
- [x] Model privacy classification and a one-time, contextual warning/acknowledgement gate before first use.
- [ ] Provider/product-specific wording: distinguish consumer apps from API/business offerings and endpoint-specific incidents.
- [x] Local reversible text redaction for configured personal identifiers and detected sensitive values before provider submission.
- [x] Keep redaction mappings local; restore placeholders in displayed responses where safe, without corrupting code/patches.
- [x] State clearly that detection/redaction is best-effort and does not make transmission risk-free.
- [ ] OCR/vision-aware privacy scanning for image attachments; image contents require a separate explicit grant and are currently sent unscanned.

## M3 — Repository understanding

- [x] Initial trusted-workspace boundary and path safety checks; full ignore-rule support remains.
- [x] Bounded file listing, literal text search, targeted reads, and read-only Git status tools.
- [ ] Git diff, repository map, and full ignore-rule support.
- [ ] Context selection with visibility into what is sent to the model.
- [ ] Diagnostics and language-server integration as an optional capability.

## M4 — Agent/tool runtime

- [x] Initial bounded provider-neutral function-tool loop, adapter translations, argument validation, call budgets, and visible tool-action transcript.
- [x] Read-only repository tools, exact-unique-snippet edits with stale-content checks, and permission-gated shell command execution.
- [ ] Prompt-injection defenses for repository and tool output; output limits and secret redaction.
- [x] Initial deterministic permission rules for Accept Edits, Auto, Accept Minimal, Accept Everything, and Plan, with action approval prompts.
- [x] Model recommendations cannot grant permissions beyond the harness's deterministic rules.

## M5 — Trustworthy edit and verification loop

- [x] Reviewable exact-snippet changes and stale-edit detection; patch-based edits, recovery/checkpoints, and undo remain.
- [x] Run commands with displayed shell/cwd, bounded execution time/output, exit status, and captured output.
- [x] Feed relevant failures into the bounded tool loop; user can review and stop the turn.
- [ ] Final summary of changed files, checks, and remaining issues.

## M6 — Dynamic workflows and effort orchestration

Dynamic workflows are runtime-authored orchestration for complex coding tasks: decompose into subtasks, fan out independent work to parallel subagents, check/refute results, iterate toward a validated outcome, and coordinate a single final result. Long-running execution must persist progress and resume after interruption.

- [ ] Workflow plan representation with dependencies, checkpoints, and inspectable progress.
- [ ] Scoped subagents with isolated task instructions and permission inheritance/limits.
- [ ] Parallel scheduling, result validation/refutation, convergence, retries, and cancellation.
- [ ] Persistence/recovery for long-running jobs and an auditable workflow trace.
- [ ] Resource budgets: depth, agent count, wall time, tokens, and cost estimates/actuals.
- [ ] Low/Medium/High/Max map to supported provider-native normal effort levels.
- [ ] XHigh (between High and Max) is a standalone xhigh effort setting without workflow orchestration.
- [ ] Super = xhigh effort plus dynamic workflows; Extreme = max effort plus dynamic workflows.
- [ ] Warn and require explicit confirmation before enabling/starting potentially costly Extreme workflows.
- [ ] If provider/model lacks a capability, show the fallback and do not imply it was honored.

## M7 — Extensibility and integrations

- [ ] Versioned tool/plugin contract with explicit permissions.
- [ ] Optional MCP support through the same policy and audit layer.
- [ ] Git conveniences for branch and commit preparation; never silently commit or push.
- [ ] Project hooks and reusable coding workflows.

## M8 — Hardening and release

- [ ] Cross-platform security review: path traversal, process execution, secrets, network, and persistence.
- [ ] Unit, integration, provider-conformance, and end-to-end tests.
- [ ] Windows/macOS/Linux behavior throughout (not deferred to release): terminal compatibility, accessibility, and performance.
- [ ] Later/second-class porting track: HarmonyOS/OpenHarmony (including laptop devices), Haiku OS, FreeBSD, and NetBSD; validate Rust target availability, system dependencies, terminal behavior, packaging, and feature fallbacks separately.
- [ ] LLVM-only native compilation (Clang/clang-cl, LLD, LLVM-MinGW where appropriate); no GCC compiler or GCC toolchain.
- [ ] Portable process abstraction with explicit shell profiles (PowerShell/CMD and POSIX shells), quoting tests, working-directory handling, and cancellation.
- [ ] OS credential-store adapters, home/config/data directory conventions, filesystem permissions, path casing/symlink behavior, and atomic writes per platform.
- [ ] Config/session migrations, installer/distribution, docs, troubleshooting, and privacy controls.

## Privacy warning policy (initial requirements)

- Recognize aliases/keywords such as Gemini/Google and GLM/Z.ai/z ai, but classify the actual provider, product, endpoint, and account/API context whenever possible.
- When selecting a flagged model (and before first outbound use if configured non-interactively), show a concise explanation and require an explicit affirmative picker choice; Cancel returns without sending anything.
- Remember acknowledgement for that provider/product identity so routine future use is not interrupted. Provide a settings command to review/reset acknowledgements.
- Never label all Google/Gemini or all GLM/Z.ai endpoints as equivalent. The warning must explain the relevant scope and link to current policy/incident information.
- With local redaction enabled for flagged services, replace configured personal identifiers (for example, the user's name) and detectable sensitive values with stable opaque tokens before prompt/context leaves the machine. Keep the token map local and out of model-visible context.
- Redact at the outbound boundary, including prompts, repository context, and tool output; don't mutate source files. For model-generated prose, restore known tokens for display. For patches/code, use a patch-aware review/restore path and avoid unsafe blind replacement.
- Offer a review of redactions and allow user overrides. Document gaps: names and sensitive data can be missed, inferred, or revealed by surrounding context.
