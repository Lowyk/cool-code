# Changelog

All notable changes are listed here, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Workflow sizes: Settings → General → Dynamic workflows now chooses the most subagents a turn
  may start (Off, Small 5, Medium 15, Big 30, Large 50, Massive 100, Extreme 200, or a Custom
  number up to 500). Ultimate may use the whole size, Super half and lower levels with workflows a
  quarter. Massive, Extreme and Custom sizes above 100 are confirmed once.
- An "At once" setting (8 by default, 1 to 32) for how many subagents run at the same time.
- Low, High and Max effort for DeepSeek V4 (`deepseek-flash`, `deepseek-v4-pro`) and Kimi K3
  (`kimi-k3`, and `k3`, `k3-256k` and `kimi-for-coding` on Kimi Code), sent as `reasoning_effort`.

### Changed

- The Dynamic workflows switch became a size. Settings files with `dynamic_workflows = true` load
  as Medium, `false` as Off. Medium keeps Super's old limits (4 per call, 8 per turn); Ultimate on
  Medium allows 5 per call and 15 per turn (it was 6 and 20).
- Explorer subagents in one call wait for a free place instead of all starting at once.

## [0.1.0] - 2026-10-08

The first release.

### Added

- A Windows installer (`coolcode-v0.1.0-windows-x64-setup.exe`) that installs for your user
  without an administrator prompt and can add `coolcode` to your PATH, and an install script for
  macOS and Linux that checks the download against its checksum.

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
- `coolcode run` runs one turn without the interface, for scripts and CI (`--json` available).
- Auto mode: every command, edit and new file goes to guard models you choose in Settings → Auto
  Mode (recommended small models are listed first). A guard answers yes or no; a no asks you with
  the reason. Guards are tried in order, so one running out of usage hands over to the next, and
  with none available the action does not run and the model is told Auto mode is unavailable.
  Dangerous commands and secret files still always ask you. Auto is greyed out until a guard is set
  up. Llama Prompt Guard models can be added as injection scanners, which never approve anything.
- Manual mode: asks before every edit, new file and command.
- Light mode, and live previews in Settings → Appearance (themes in their own colours, the
  backdrop options, and the effort-name animation).
- Shift+Enter or Alt+Enter starts a new line in the prompt.
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

- The program is now called `coolcode` (it was `harness`). Settings, sessions and saved keys are
  unchanged.
- Project data (trusted folders and per-project choices) lives in `~/.coolcode/`, not inside
  projects. Folders trusted by older versions must be trusted again once.
- Series names such as `/model fable` respect each provider's automatic-switching setting.
- Notices raised in Settings are shown in the Settings footer.
- The page after a ChatGPT sign-in looks like a Windows 10 console window.

### Fixed

- The `@` file list showed only eight matches and could not scroll; it now keeps up to 200 and
  scrolls with the highlight.
- Pasting several lines sent the message at the first line break.
- A command that left a background process running (a dev server, `start /b`) could make the
  turn wait forever; it now waits at most two seconds for the output to close.
- A crash left the terminal in raw mode; the terminal is restored before the message is shown.
- Pasting an API key with a non-ASCII character near its start crashed the harness.
- Plan mode no longer blocks read-only tools.
