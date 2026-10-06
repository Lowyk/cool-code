# Settings rework, model picker, and provider command

Date: 2026-10-06
Status: draft, pending review

## Goal

Replace the tabbed settings popup with a full-screen settings view in which every setting is editable in place, add a `/model` quick picker, and add a `/provider` command. Keyboard behavior must be consistent across all of these surfaces.

## Problems addressed

- The settings popup is cramped and visually flat.
- The General tab displays model, effort, and mode, but cannot change them.
- Navigation keys differ between tabs and are not discoverable.
- Provider and model management is buried in nested forms.
- `/model` without an argument prints a usage line instead of offering a choice.

## Non-goals

- No change to provider adapters, credential storage, model resolution rules, or permission policy.
- No visual redesign of the effort picker or background (tracked separately).
- No new settings beyond those that exist today.

## User-facing design

### Full-screen settings

`/settings` opens a full-screen view: a sidebar of sections on the left, the selected section's content on the right, and a footer that lists the keys valid in the current context.

Sections: General, Providers, Models, Auto-switch, Privacy.

- **General:** model, effort, permission mode, and workspace trust. Each row is editable: Enter opens the relevant picker or toggle.
- **Providers:** a list of providers (with `●` for the default and tags for `default` and `auto`), followed by an `+ Add provider` row. The selected provider's details (adapter, base URL, masked API key, model count) appear below the list. Single-key actions: `d` set default, `a` toggle auto-switch, `e` edit, `x` delete (with confirmation).
- **Models:** a flat list of every model across non-draft providers, showing display name, registered ID, and provider. Supports add, rename, and remove.
- **Auto-switch:** model chains, with the same create, edit, and activate behavior as today.
- **Privacy:** workspace trust, custom redaction values, and acknowledgement reset, with the same behavior as today.

Below roughly 70 columns, the sidebar collapses to a one-line section switcher above the content.

### `/model` quick picker

`/model` with no argument opens a popup over the chat. It lists models grouped by provider, marks the active model with `●`, and dims draft or disabled providers. Typing filters by display name, model ID, or provider name. Enter activates that exact provider and model pair. Esc closes without changes. `/model <id>` keeps its current behavior.

### `/provider`

`/provider` opens the full-screen settings on the Providers section. `/settings` opens General; `/chain` opens Auto-switch.

### Consistent keys

| Key | Action |
| --- | --- |
| Up / Down | Move selection |
| Enter | Edit or select |
| Left / Right | Switch between sidebar and content |
| Esc | Back out of an edit; close the view from the top level |
| Printable characters | Filter the focused list (where filtering applies) |
| Letter shortcuts | Section-specific actions, always listed in the footer |

In text-entry fields, printable characters edit the field instead of filtering or triggering shortcuts.

## Architecture

| Unit | Responsibility |
| --- | --- |
| `tui/widgets/list.rs` | Reusable selectable list: grouping headers, dimmed rows, filtering, scrolling, selection |
| `tui/pickers/model.rs` | `/model` quick picker state, key handling, and drawing |
| `tui/settings/mod.rs` | `SettingsView` state, section switching, frame layout, footer, small-terminal fallback |
| `tui/settings/{general,providers,models,auto_switch,privacy}.rs` | One section each: `draw` and `handle_key` |

`SettingsView { section, focus, … }` replaces the scattered settings fields on `App` (`settings_menu`, `settings_tab`, `provider_index`, `provider_form`, `chain_form`, `chain_index`). The existing provider and chain form logic in `forms.rs` moves into the matching section modules. The old tab rendering in `render/settings.rs` is deleted once every section is ported.

## Invariants carried over

- API keys are stored only in the OS credential store and are always masked on screen.
- Deleting a provider removes its chain references and selects a valid replacement default.
- Draft providers are never used for automatic model resolution.
- Existing slash commands keep their behavior.

## Testing

- `TestBackend` render tests for each settings section, the collapsed small-terminal layout, and the model picker.
- Key-handling tests: filtering, selection, Enter activation, delete confirmation, and Esc behavior in edit versus top level.
- The existing provider, chain, and API-key-masking tests are ported with unchanged assertions.

## Delivery order

1. Shared list widget.
2. `/model` quick picker.
3. Settings shell and General section.
4. Providers and `/provider`.
5. Models.
6. Auto-switch and Privacy; delete the old tab code.

Each step leaves the application working and all tests green.
