# TUI restructure design

Date: 2026-10-06
Status: draft, pending review

## Goal

Split `src/tui.rs` (5305 lines) into focused modules with no change in behavior, then move the agent turn loop and approval policy out of the UI layer. This establishes the structure on which later TUI improvements (see ROADMAP M1) will be planned.

## Non-goals

- No new features, visual changes, or keybinding changes.
- No changes to `provider.rs`, `tools.rs`, `secrets.rs`, or the public CLI surface.
- No new dependencies.

## Success criteria

- `cargo check` and `cargo test` pass after every commit, with the test count never decreasing from the 37-test baseline.
- Rendered output and runtime behavior are identical to the baseline.
- No resulting source file exceeds roughly 800 lines, excluding tests.
- Each module has a single stated responsibility.

## Current state

`tui.rs` combines six concerns: application state and slash-command handling, the agent turn loop and tool approvals, the terminal event loop and context loading, rendering, effort-slider and wordmark animation, and model/provider resolution helpers. About 500 lines are unit tests that access private items through `use super::*`.

The architecture document defines the agent runtime and policy engine as layers separate from the CLI/TUI. In the current code they are not.

## Approach

Two phases, each independently verifiable.

### Phase 1: mechanical split inside `tui/`

Convert `src/tui.rs` to a `src/tui/` directory. Move code without altering logic. Items keep their current names and signatures; visibility widens only as far as `pub(super)` where cross-module access requires it.

| Module | Contents |
| --- | --- |
| `mod.rs` | `run`, terminal setup/restore, `run_app` event loop |
| `state.rs` | `App`, draft structs, transcript types, `PendingEvent`, constants (`LEVELS`, `MODES`, `PROVIDER_PRESETS`, system prompt) |
| `commands.rs` | `App::submit`, `dispatch_user_message`, `poll_response`, slash-command handlers |
| `models.rs` | model and chain activation, model resolution, display-name helpers |
| `forms.rs` | provider and chain form handlers, settings key handling, provider save/delete |
| `context.rs` | `COOL.md` loading, trust marker, `build_user_message` |
| `render/mod.rs` | `draw`, layout helpers, input wrapping |
| `render/settings.rs` | `draw_settings`, `draw_chain_form` |
| `render/dialogs.rs` | trust, privacy, approval, extreme-confirmation, and picker dialogs |
| `effort.rs` | effort slider rendering, animation, color math |
| `wordmark.rs` | wordmark glyphs and gradient |

Unit tests move into the module that owns the code under test. Tests spanning modules stay in `mod.rs`.

The agent loop (`run_agent_turns`, `execute_agent_tool`, the approval functions) is moved as one block into `agent_bridge.rs` in this phase, unchanged, so Phase 2 starts from a clean boundary.

### Phase 2: extract agent and policy

- `src/agent.rs`: the bounded turn loop and tool execution (`run_agent_turns`, `execute_agent_tool`, budgets). It communicates with the UI through the existing `PendingEvent` channel and approval request type; it must not import ratatui or crossterm.
- `src/policy.rs`: the deterministic permission rules (`auto_approve_create`, `auto_approve_edit`, `auto_approve_command`) and the mode table. These are safety-critical and get dedicated unit tests covering each permission mode, including edge cases the existing single test does not cover.

The channel and approval types move to a neutral location so `agent.rs` does not depend on `tui`.

## Constraints

- The Rust edition is 2024; follow the existing formatting (`cargo fmt`).
- Commit after each module move, with tests green, so any regression bisects to one move.
- Behavior-preserving: if a move requires a logic change to compile, stop and record it rather than changing it silently.

## Risks

| Risk | Mitigation |
| --- | --- |
| Tests rely on private access via `use super::*` | Move tests with their code; widen to `pub(super)` only when necessary |
| `App` is a large struct shared by many handlers | Keep `App` in `state.rs`; handlers become `impl App` blocks in their own files |
| Silent behavior drift in a "pure" move | Compare `cargo test` results per commit; review each diff as moves only |
| Phase 2 type relocation creates import cycles | Define shared types in the neutral module first, then redirect imports |

## Out of scope for this spec

UX improvements, streaming, new tests beyond policy rules, and CI setup. These will be planned after the restructure.
