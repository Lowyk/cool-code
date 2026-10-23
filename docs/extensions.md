# Skills, plugins, mods and MCP servers

Four ways to extend Cool Code without changing it. None of them can change the permission mode,
approve an action or get around a check: skills and plugin commands are text for the model, mods
only receive events and send back text for the screen, and an MCP server's tools ask you before
every call.

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

### Updating a plugin

`/plugin update <name>` fetches the plugin again from where it was installed from (Cool Code remembers
the Git address or folder; for a plugin installed before that, give one: `/plugin update <name>
<source>`). The source must hold a plugin of the same name. The review shows the version change,
what changed in the commands, mods and MCP servers (new, changed or removed, each with its full
command line), and everything the new version has. Until you press **y** the installed version
is not touched; on **y** its folder is set aside, the new one moved in (and the old one moved
back if that fails), the mods and servers you were shown are approved, and the approvals and secrets of those
the update dropped are forgotten.

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

## MCP servers

An MCP server is a program that gives the model more tools (a database, a browser, a search
service). Cool Code starts it as a child process and talks to it over its standard input and
output (the "stdio" transport of the Model Context Protocol). Servers over HTTP are not
supported yet.

A server is configured in `~/.coolcode/mcp.toml`, in a trusted project's `.coolcode/mcp.toml`, or
as a `[[mcp]]` entry of a plugin's `plugin.toml`:

```toml
[[server]]
name = "files"                       # up to 24 lowercase letters, digits, - and _
description = "Reads the notes folder"
command = "npx"                      # a name on PATH, an absolute path, or ./a/path in its folder
args = ["-y", "@example/notes-server", "/home/me/notes"]
env = ["NOTES_TOKEN"]                # names only; the values are never in a file
```

`/mcp add <name> <command> [arguments…]` writes an entry for you.

**Approval.** A server runs only after you approved it: in **Settings → Plugins** (MCP servers),
or `/mcp approve <name>`, which shows the exact command, where it runs and which secrets it asks
for. The approval is stored with a hash of the entry, so any change to the command, arguments,
secrets or folder has to be approved again. A plugin's servers are shown with the plugin's
install review and approved with it. A project's `mcp.toml` is only read in a folder you trust.

**Secrets.** The variables in `env` are the only secrets a server receives. Their values are set
in Settings → Plugins (the `s` key, or `/mcp secret <name>`), typed hidden, and kept in the OS
credential store like API keys. The server's environment otherwise has only the harmless
variables mods get (`PATH`, `HOME`, the locale, the temporary folder, the Windows system
variables), so API keys in your environment are not passed on.

**Tools.** Each tool appears to the model as `mcp__<server>__<tool>` (names are cleaned to what
model APIs accept and made unique; at most 64 tools per server). Every call asks you first,
showing the server, the tool and its arguments, except in **Accept Everything**. In **Auto**
mode the guard models decide each call, and with none available it does not run. **Plan** mode
refuses MCP tools, and subagents never get them. A server's own claim that a tool "only reads" is
shown to you but never skips the question.

**Results.** What a tool returns is handed to the model as data from a third-party program, with
terminal escape sequences and control characters removed and cut to 24 KiB. Only text is kept
(images, audio and files become a short note). If a Prompt Guard scanner is set up in Auto mode
and flags a result, the model is told so.

**Limits.** A server has 30 seconds to start and list its tools, a call 60 seconds (Esc stops a
call sooner). It may print lines up to 1 MiB and 64 MiB in all, then it is stopped. It can only
answer `ping`: requests for sampling or file roots are refused. A server that crashes is marked
failed and is not restarted by itself (`/mcp restart <name>`). Servers start with the interface
and are stopped when Cool Code exits; `coolcode run` does not start them.

`/mcp` lists the servers and their state; `/mcp enable|disable|restart|remove <name>` manage
them. The status line shows `MCP 2/3` (servers up out of those switched on).
