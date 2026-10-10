# Skills, plugins and mods

Three ways to extend Cool Code without changing it. None of them can change the permission mode,
approve an action or get around a check: skills and plugin commands are text for the model, and
mods only receive events and send back text for the screen.

## Skills

A skill is a folder with a `SKILL.md` and, optionally, helper files. The format is the one Claude
Code uses, so its skills work here too.

```markdown
---
name: release-notes
description: Write release notes from the commits since the last tag
---

# Release notes

1. Run `git log` from the last tag ...
```

- The frontmatter needs `name` (lowercase letters, digits, `-` and `_`, at most 64 characters)
  and `description` (at most 1024 characters). Quoted values, `|` and `>` blocks and other keys
  (such as `allowed-tools` or `metadata`) are accepted; other keys are ignored.
- `SKILL.md` may be at most 64 KiB.

Skills are read from, in this order (the first skill with a name wins):

1. `~/.coolcode/skills/<name>/SKILL.md`, always;
2. `<project>/.coolcode/skills/<name>/SKILL.md`, only in a trusted folder, and only if the folder
   does not lead outside the project;
3. the skill folders of enabled plugins;
4. `~/.claude/skills/<name>/SKILL.md`, only with **Settings > General > Load Claude skills** on.

The system prompt lists only names and descriptions (at most 30; the number of the others is
given). The model loads a skill with the read-only `use_skill` tool, which is offered in every
permission mode, Plan included, while skills are installed. It returns the instructions and the
list of helper files. A helper file inside the workspace is read with `read_file`; one outside it
is read with `use_skill` and its `file` argument, which only reaches files inside that skill's
folder (no `..`, no absolute paths, no links that lead out, nothing that looks like a secret,
512 KiB at most).

`/<name> [text]` asks the model to load the skill and adds your text. Skills need the workspace
tools, so this works in a trusted folder. A skill can never take the name of a built-in command.

## Plugins

A plugin is a folder or Git repository with a `plugin.toml`:

```toml
name = "reviewer"            # lowercase letters, digits, - and _
version = "1.2.0"
description = "Code review helpers"
skills = ["skills"]          # folders inside the plugin that hold skill folders

[[commands]]
name = "review"
description = "Review the staged changes"
prompt = "Review the staged changes. Focus on $ARGUMENTS."

[[mods]]
name = "status"
command = "python3"
args = ["mods/status.py"]
events = ["turn_finished"]
```

- `/review the tests` sends the prompt with `$ARGUMENTS` replaced by `the tests`; without
  `$ARGUMENTS`, your text is added after a blank line. Built-in commands and skills win over a
  plugin command with the same name.
- Unknown keys are refused, so a manifest cannot hide anything. Every path it names has to stay
  inside the plugin.

`/plugin install <git-url or folder>` clones the repository (`git clone --depth 1`, no prompts)
or copies the folder (without `.git` and without links) into a staging folder under
`~/.coolcode/plugins`, checks it, and shows everything it adds: its skills, its commands with the
start of their prompts, and every mod with its exact command line, folder and events. Only **y**
moves it to `~/.coolcode/plugins/<name>` and approves the mods you were shown; **n** deletes the
staging folder. An installed plugin is never overwritten: remove it first.

`/plugin list` shows what is installed and `/plugin remove <name>` deletes it. **Settings >
Plugins** switches plugins off and on and removes them.

## Mods

A mod is a program that watches Cool Code. It comes with a plugin or lives in
`~/.coolcode/mods/<name>/mod.toml` (the folder has the mod's name):

```toml
name = "clock"
description = "Shows the time of the last answer"
command = "python3"          # a name on PATH, an absolute path, or ./a/path inside the folder
args = ["clock.py"]
events = ["turn_finished"]
```

A mod runs only after you approved it: at install time for a plugin's mods, in **Settings >
Plugins** for the others. The approval is stored with a hash of the whole manifest and the
mod's folder, so a changed manifest has to be approved again. Approved mods can be
switched off there.

Mods are started when Cool Code starts, off the interface thread, in their own folder, with only
a few environment variables (`PATH`, `HOME`, the locale, the temporary folder and the Windows
system variables) plus `COOLCODE_MOD=1`, so API keys in your environment are not passed on. They
are stopped when Cool Code exits.

### Protocol

Events arrive on standard input, one JSON object per line, only those the mod asked for:

| Event | Fields |
| --- | --- |
| `session_started` | `version`, `project` (the folder's name), `mode`, `model` |
| `prompt_submitted` | `chars` (the length of the message, never its text) |
| `tool_started` | `tool` (the tool's name; a shell command is always `run_command`, never its command line) |
| `tool_finished` | `tool`, `ok` |
| `turn_finished` | `outcome`: `done`, `failed` or `cancelled` |

Each line has an `event` field with the event's name, for example
`{"event":"turn_finished","outcome":"done"}`.

A mod writes one JSON object per line on standard output:

- `{"status": "text"}` sets its part of the status line (`""` or `null` clears it);
- `{"toast": "text"}` shows a notice.

Text is cleaned of control characters (escape sequences, line breaks) and direction overrides,
and cut to 40 characters for a status and 160 for a notice. Anything else on a line, such as
`{"approve": true}`, is not a message: the first such line is reported once and the rest are
ignored. Mods cannot approve, block or change anything.

Limits: lines up to 4 KiB, 20 messages a second (the rest are dropped), 1 MiB of output in all
(then the mod is stopped). A mod that does not read its events has new events dropped instead of
slowing Cool Code down. A mod that crashes is reported with its exit status; the others keep
running.
