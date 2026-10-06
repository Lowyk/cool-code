# TUI Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `src/tui.rs` into focused modules without changing behavior, then move the agent loop and permission policy out of the UI layer.

**Architecture:** Phase 1 converts `src/tui.rs` into a `src/tui/` directory and moves code leaf-first, one module per commit, keeping every test green. Phase 2 lifts the agent loop into `src/agent.rs` and the permission rules into `src/policy.rs`, and adds characterization tests for the policy rules.

**Tech Stack:** Rust 2024, ratatui 0.29, crossterm 0.28, cargo test.

**Spec:** `docs/superpowers/specs/2026-10-06-tui-restructure-design.md`

## Global Constraints

- Behavior-preserving: no logic, rendering, keybinding, or string changes. If a move cannot compile without a logic change, stop and record it instead of changing it.
- Baseline is commit `76e7593`: `cargo check` clean, `cargo fmt --check` clean, `cargo test` reports `37 passed`.
- After every commit: `cargo fmt --check` clean, `cargo check` clean, `cargo test` reports at least `37 passed` (Phase 2 raises the count only by the new policy tests).
- No new dependencies. No changes to `provider.rs`, `tools.rs`, `secrets.rs`, or CLI behavior.
- Visibility: widen only to `pub(super)` inside `src/tui/`; items shared outside `tui` use `pub(crate)`. Never use bare `pub`.
- Size target: no non-test source file above roughly 800 lines.
- Commit messages and code comments use neutral professional language, conventional subjects in lowercase (`refactor(tui): ...`). Every commit ends with the two attribution trailers from the session reminder.
- Locate code by function name. Line numbers refer to baseline `76e7593:src/tui.rs` and shift as code moves.

## Review Focus

Policy rules guard what runs without user approval. The spec's silence on these inputs does not make them safe to break; each has a pinning test in Task 13.

- Unknown, empty, or mis-cased mode strings (`""`, `"AUTO"`, `"Plan"`, `"yolo"`): approve nothing.
- Sensitive paths in `auto` mode (`.env`, `.ENV.local`, `config\Secrets.json`, `src/my_credentials.rs`): require approval; ordinary paths do not.
- Size boundary in `auto` mode: content of exactly 2048 bytes is auto-approved, 2049 is not (edits: either `before` or `after` over the limit).
- Allowlist near-misses (`cargo test --release`, `cargo test && echo x`, `cargo  test`): never auto-approved; surrounding whitespace and uppercase (` CARGO TEST `) are accepted, which is current behavior.
- `plan` mode: nothing is auto-approved, including allowlisted commands and file creation.

---

## Module allocation (authoritative for Phase 1)

| Module | Items moved verbatim from `tui.rs` |
| --- | --- |
| `state.rs` | `App` and its `impl` helpers `new`, `finish_command`, `choose_effort`, `apply_effort`, `apply_mode`; `TranscriptKind`, `TranscriptEntry`, `PendingEvent`, `ToolApproval`, `PrivacyPrompt`, `SettingsTab`, `ProviderDraft`, `ModelDraft`, `ChainDraft`, `ProviderPreset`; consts `LEVELS`, `MODES`, `PROVIDER_PRESETS`, `CORE_SYSTEM_PROMPT_VERSION`, `CORE_SYSTEM_PROMPT`; fns `mode_alias`, `mode_label`, `adjacent_settings_tab`, `edit_string` |
| `wordmark.rs` | `cool_code_wordmark` (with inner `glyph`), `logo_blend_color`, `ice_gradient_color` |
| `effort.rs` | `draw_effort_picker`, `effort_name`, `effort_style`, `bar_segment`, `height_glyph`, `selected_bar_height`, `selected_bar_color`, `effort_rgb`, `animated_effort_color`, `scale_rgb`, `scale_color`, `blend_color`, `gradient_name`, `effort_label` |
| `context.rs` | `read_cool_file`, `read_user_instructions`, `workspace_is_trusted`, `build_user_message`, `App::set_workspace_trusted` |
| `models.rs` | `App::select_model`, `activate_model`, `activate_chain`, `toggle_chain`; fns `find_model_matches`, `find_model_matches_all`, `available_chain_models`, `slug`, `model_id_for_profile`, `resolve_model_reference`, `model_author_matches`, `selected_model_name`, `model_display_for_profile`, `model_name` |
| `forms.rs` | `App::edit_provider`, `delete_provider`, `save_provider`, `handle_provider_form`, `handle_settings_key`, `edit_chain`, `save_chain`, `handle_chain_form`; fns `unique_provider_alias`, `remove_provider_profile`, `provider_focus_layout` |
| `commands.rs` | `App::submit`, `dispatch_user_message`, `poll_response`, `run_readonly_tool`; fn `format_provider_error` |
| `render/mod.rs` | `draw`, `centered_rect`, `wrap_input_text`, `input_visual_lines`, `input_prompt_height` |
| `render/settings.rs` | `draw_settings`, `draw_chain_form`, `chain_field_line` |
| `render/dialogs.rs` | `draw_tool_approval`, `draw_privacy_confirmation`, `draw_workspace_trust_prompt`, `draw_model_provider_picker`, `draw_mode_picker`, `draw_extreme_confirmation` |
| `agent_bridge.rs` | `run_agent_turns`, `execute_agent_tool`, `request_plan_approval`, `request_tool_approval`, `summarize_tool_result`, `auto_approve_create`, `auto_approve_edit`, `auto_approve_command` |
| `mod.rs` (remainder) | `run`, `setup_terminal`, `restore_terminal`, `run_app`, `mod` declarations |

Because `impl App` blocks move into sibling modules, `App` and the draft structs in `state.rs` expose their fields as `pub(super)`.

## Move procedure (steps M1 to M5, referenced by Tasks 2 to 11)

- [ ] **M1:** Create the target file; move the listed items verbatim; add `mod <name>;` to `src/tui/mod.rs`; add the `use` lines each file needs (`use super::*;` is acceptable in intermediate commits, but prefer explicit imports).
- [ ] **M2:** Move each listed test, unchanged, into a `#[cfg(test)] mod tests` in the target file.
- [ ] **M3:** Run `cargo fmt`, then `cargo check`. Expected: no errors or warnings introduced.
- [ ] **M4:** Run `cargo test`. Expected: `37 passed; 0 failed`.
- [ ] **M5:** Commit with the task's message.

---

## Phase 1: mechanical split

### Task 1: Convert `tui.rs` to a directory module

**Files:**
- Rename: `src/tui.rs` to `src/tui/mod.rs`

**Produces:** the `src/tui/` directory; `mod tui;` in `src/main.rs` resolves unchanged.

- [ ] **Step 1:** `git mv src/tui.rs src/tui/mod.rs` (create `src/tui/` first).
- [ ] **Step 2:** `cargo check`, then `cargo test`. Expected: clean, `37 passed`.
- [ ] **Step 3:** Commit `refactor(tui): convert tui module to a directory`.

### Task 2: Extract `wordmark.rs`

**Files:** Create `src/tui/wordmark.rs`; modify `src/tui/mod.rs`.
**Produces:** `pub(super) fn cool_code_wordmark(elapsed: f32) -> Vec<Line<'static>>`, `pub(super) fn logo_blend_color(left: Color, right: Color, amount: f32) -> Color`, `pub(super) fn ice_gradient_color(position: f32) -> Color`.
**Tests moved:** `welcome_wordmark_is_wide_and_has_a_visible_bloom_phase`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract wordmark module`.

### Task 3: Extract `effort.rs`

**Files:** Create `src/tui/effort.rs`.
**Consumes:** `Effort`, `LEVELS` (remain in `mod.rs` until Task 4).
**Tests moved:** `effort_picker_renders_horizontal_levels_and_xhigh_mapping`, `advanced_effort_highlights_change_color_over_time`, `effort_bar_height_profiles_match_the_selected_tier`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract effort slider module`.

### Task 4: Extract `state.rs`

**Files:** Create `src/tui/state.rs`.
**Produces:** all state types and consts listed in the allocation table; every `App` field and draft-struct field becomes `pub(super)`.
**Tests moved:** none.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract application state module`.

### Task 5: Extract `context.rs`

**Files:** Create `src/tui/context.rs`.
**Tests moved:** `workspace_trust_marker_is_scoped_to_its_folder`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract workspace context loading`.

### Task 6: Extract `models.rs`

**Files:** Create `src/tui/models.rs`.
**Tests moved:** `model_names_apply_series_styling_and_number_runs`, `model_author_namespace_is_not_a_provider_selector`, `author_qualified_model_resolves_across_multiple_provider_profiles`, `auto_switch_provider_wins_over_default_provider_model_collision`, `status_uses_configured_model_display_name`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract model and chain resolution`.

### Task 7: Extract `forms.rs`

**Files:** Create `src/tui/forms.rs`.
**Tests moved:** `settings_left_arrow_moves_to_previous_tab`, `duplicate_adapter_profiles_get_distinct_aliases`, `deleting_provider_cleans_references_and_selects_a_valid_replacement`, `provider_settings_mask_api_key_input`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract provider and chain forms`.

### Task 8: Extract `commands.rs`

**Files:** Create `src/tui/commands.rs`.
**Tests moved:** `slash_command_is_rendered_as_user_input_and_plain_command_output`, `clear_empties_the_transcript_and_restores_the_welcome_layout`, `completed_answer_returns_history_to_bottom`, `provider_errors_are_transcript_output_with_status_and_api_message`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract slash-command handling`.

### Task 9: Extract `render/mod.rs` and `render/settings.rs`

**Files:** Create `src/tui/render/mod.rs`, `src/tui/render/settings.rs`.
**Produces:** `pub(super) fn draw(frame: &mut ratatui::Frame<'_>, app: &App, animation_tick: usize)`; `centered_rect` is `pub(super)` for use by `dialogs.rs` and `effort.rs`.
**Tests moved:** `long_prompt_wraps_and_grows_the_input_area`, `prompt_cursor_tracks_the_explicit_continuation_lines`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract render root and settings views`.

### Task 10: Extract `render/dialogs.rs`

**Files:** Create `src/tui/render/dialogs.rs`.
**Tests moved:** `privacy_dialog_offers_separate_image_consent`.

- [ ] Follow M1 to M5. Commit `refactor(tui): extract dialog and picker rendering`.

### Task 11: Extract `agent_bridge.rs` and verify Phase 1

**Files:** Create `src/tui/agent_bridge.rs`; modify `src/tui/mod.rs`.
**Tests moved:** `permission_modes_have_deterministic_edit_and_command_rules`.

- [ ] Follow M1 to M5. Commit `refactor(tui): isolate agent loop and approval policy`.
- [ ] **Step 6: Verify the restructure is moves only.** Run `git show 76e7593:src/tui.rs | sort > base.txt` and `cat src/tui/*.rs src/tui/render/*.rs | sort > now.txt` in the scratchpad directory, then `diff base.txt now.txt`. Expected: differences consist only of `use`/`mod` lines, `pub(super)` visibility changes, and `#[cfg(test)]`/`mod tests` wrappers; any other differing line must be explained or fixed.
- [ ] **Step 7:** Confirm `src/tui/mod.rs` contains only `mod` declarations, `run`, `setup_terminal`, `restore_terminal`, and `run_app`; `wc -l src/tui/*.rs src/tui/render/*.rs` shows no non-test file far above 800 lines; `git push`.

---

## Phase 2: extract agent and policy

### Task 12: Extract `policy.rs`

**Files:** Create `src/policy.rs`; modify `src/main.rs` (`mod policy;`), `src/tui/agent_bridge.rs`, `src/tui/state.rs`, `src/tui/render/dialogs.rs`.

**Produces:**
- `pub(crate) const MODES: [(&str, &str); 5]` (moved from `tui/state.rs`).
- `pub(crate) fn auto_approve_edit(permission_mode: &str, proposal: &crate::tools::EditProposal) -> bool`
- `pub(crate) fn auto_approve_create(permission_mode: &str, proposal: &crate::tools::CreateProposal) -> bool`
- `pub(crate) fn auto_approve_command(permission_mode: &str, command: &str) -> bool`

- [ ] **Step 1:** Move the three functions and `MODES` verbatim; redirect call sites and imports in the `tui` modules; move `permission_modes_have_deterministic_edit_and_command_rules` into `policy.rs`.
- [ ] **Step 2:** `cargo fmt`, `cargo check`, `cargo test`. Expected: `37 passed`.
- [ ] **Step 3:** Commit `refactor: extract permission policy module`.

### Task 13: Characterization tests for policy rules

**Files:** Modify `src/policy.rs` (`#[cfg(test)] mod tests`).

The rules already exist, so these tests are expected to pass on first run; a failure means the move changed behavior or the test misstates it. Build proposals with `EditProposal { relative_path, original, updated, change_summary, before, after }` and `CreateProposal { relative_path, content }`.

- [ ] **Step 1: Write these tests, with these assertions:**
  - `unknown_modes_approve_nothing`: for modes `""`, `"AUTO"`, `"Plan"`, `"yolo"`, edit, create, and `cargo test` are all `false`.
  - `plan_mode_approves_nothing`: edit, create, and `cargo test` are `false` under `"plan"`.
  - `accept_modes_approve_edits_and_creates_regardless_of_path_or_size`: `"accept-edits"`, `"accept-minimal"`, `"accept-everything"` return `true` for a `.env` path with 10,000-byte content.
  - `auto_mode_requires_approval_for_sensitive_paths`: `.env`, `.ENV.local`, `config\Secrets.json`, `src/my_credentials.rs` return `false` for both edit and create; `src/main.rs` returns `true`.
  - `auto_mode_size_boundary_is_2048_bytes`: create with 2048-byte content is `true`, 2049 is `false`; edit with `before` 2048 and `after` 2049 is `false`, both 2048 is `true`.
  - `command_allowlist_is_exact_after_trim_and_lowercase`: under `"auto"` and `"accept-minimal"`, `" CARGO TEST "` is `true`; `"cargo test --release"`, `"cargo test && echo x"`, `"cargo  test"` are `false`; under `"accept-edits"` all are `false`.
  - `accept_everything_approves_any_command`: `"Remove-Item -Recurse ."` is `true`.
- [ ] **Step 2:** `cargo test policy`. Expected: all new tests pass.
- [ ] **Step 3:** `cargo test`. Expected: 37 baseline tests plus the new tests pass.
- [ ] **Step 4:** Commit `test: add characterization tests for permission policy`.

### Task 14: Extract `agent.rs`

**Files:** Create `src/agent.rs`; delete `src/tui/agent_bridge.rs`; modify `src/main.rs` (`mod agent;`), `src/tui/state.rs`, `src/tui/commands.rs`, `src/tui/render/dialogs.rs`, `src/tui/mod.rs`.

**Produces:**
- `pub(crate) enum PendingEvent` and `pub(crate) struct ToolApproval { pub(crate) title: String, pub(crate) details: String, pub(crate) response: std::sync::mpsc::SyncSender<bool> }` (moved from `tui/state.rs`, variants unchanged).
- `pub(crate) fn run_agent_turns(settings: Settings, messages: Vec<provider::ChatMessage>, workspace_root: PathBuf, workspace_trusted: bool, events: &mpsc::Sender<PendingEvent>) -> Result<provider::Completion>`
- `execute_agent_tool`, `request_plan_approval`, `request_tool_approval`, `summarize_tool_result` remain private to `agent.rs`.

**Consumes:** `crate::policy::{auto_approve_edit, auto_approve_create, auto_approve_command}`.

- [ ] **Step 1:** Move the items verbatim; update `tui` imports to `crate::agent::{PendingEvent, ToolApproval, run_agent_turns}`.
- [ ] **Step 2:** Run `grep -nE "ratatui|crossterm" src/agent.rs src/policy.rs`. Expected: no matches.
- [ ] **Step 3:** `cargo fmt`, `cargo check`, `cargo test`. Expected: all tests pass.
- [ ] **Step 4:** Commit `refactor: extract agent turn loop from the tui layer`.

### Task 15: Document the layout and finish

**Files:** Modify `docs/architecture.md` (add a "Source layout" section listing `agent.rs`, `policy.rs`, `provider.rs`, `tools.rs`, `secrets.rs`, and each `tui/` module with one line of responsibility).

- [ ] **Step 1:** Add the section; `cargo fmt --check`, `cargo check`, `cargo test`, `cargo clippy` (record warnings that predate the baseline; do not fix them here).
- [ ] **Step 2:** Commit `docs: describe source layout after tui restructure`; `git push`.
- [ ] **Step 3:** Request a whole-branch review of the commits since `0547cda`, checking against the spec's success criteria.
