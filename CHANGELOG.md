# Changelog

All notable changes are listed here, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Providers: Kimi Code (membership), Moonshot, Z.ai GLM Coding Plan, DeepSeek, Mistral, xAI,
  Groq and Ollama (local) presets, and an unofficial ChatGPT Plus/Pro sign-in whose models and
  usage come from the account.
- `/usage` shows every provider's remaining usage; running-low warnings appear before a balance or
  limit runs out.
- Workflows: Super and Ultimate (and any level with the box ticked) run subagents and have the
  result reviewed. Locked until Dynamic workflows is switched on.
- Effort levels are shown per model: Super only on models with XHigh and Ultimate only on models
  with Max. GPT-5.6 and GPT-6 have Max.
- `/compact` and automatic condensing of long conversations, with a context meter in the status
  line.
- `/undo` takes back the file changes of the model's last turn without overwriting later work.
- `harness run` runs one turn without the interface, for scripts and CI (`--json` available).
- Auto mode now has a real safety check: fixed rules for dangerous commands and secret files, and a
  second model call that approves only actions that are clearly safe, asking you (with the
  reason) otherwise. `guard_model` picks a cheaper model for it.
- A `generate_image` tool for placeholder pictures, available only after you set up an image API
  (Settings → General → Image generation); each image asks first, and `/undo` can remove it.
- `@` file suggestions while typing, quoted paths with spaces, and (behind a setting, with each
  file confirmed) references to files outside the project.
- Assistant answers are drawn as Markdown (headings, emphasis, code blocks, lists, quotes, links
  and tables) instead of raw text, including while they stream.
- `/forcemodel <id>` sets a model on the default provider exactly as typed.
- Eight themes with animated backdrops; the browser page after a sign-in matches the theme.
- Opt-in saved sessions (`--resume`, `--latest`, `/resume`), a first-run setup, `CLAUDE.md` and
  `AGENTS.md` loading, and a Reset menu.
- Retries for rate limits and server errors, Anthropic prompt caching, and a larger answer limit
  for recent Claude models.
- Release builds for Windows, macOS and Linux.

### Changed

- Project data (trusted folders and per-project choices) lives in `~/.coolcode/`, not inside
  projects. Folders trusted by older versions must be trusted again once.
- Series names such as `/model fable` respect each provider's automatic-switching setting.
- Notices raised in Settings are shown in the Settings footer.

### Fixed

- Plan mode no longer blocks read-only tools.
