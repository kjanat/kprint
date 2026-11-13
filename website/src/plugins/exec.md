---
title: Exec Plugin
description: Documentation on the Exec code formatting plugin for dprint.
layout: layouts/documentation.njk
---

# Exec Plugin

Plugin that formats code via mostly any formatting CLI found on the host machine.

<div class="message is-warning">
  <div class="message-body">
    This plugin runs the commands you configure, which are not sandboxed (unlike Wasm plugins).
  </div>
</div>

The CLI has exec built in, and formats every reference to this plugin with it rather than downloading anything: any release, the latest one or none in particular, with any checksum, written any way (ex. `npm:@dprint/exec@0.7.3/plugin.json`, `npm:@dprint/exec`, `https://plugins.dprint.dev/exec-0.5.0.json`, `https://plugins.dprint.dev/dprint/dprint-plugin-exec/latest.json`, its GitHub releases or the npm package on jsDelivr or unpkg). So it works on every platform the CLI does (ex. FreeBSD), and `dprint config update` leaves such references alone. Exec always uses the built-in formatter and its host-owned configuration contract.

An `exec` section in your local configuration activates the built-in formatter directly. No entry in `plugins` or download is required. To place it among the plugins, list it as `"exec"`:

```toml
[exec]
cwd = "${configDir}"

[[exec.commands]]
command = "gofmt -s"
exts    = ["go"]
```

An explicit exec plugin reference selects the built-in formatter at that position in the formatter chain. Without a reference, built-in exec runs after the listed plugins. Commands inherited from remote configurations still require local `playWithFire` permission as described below.

Compared to the downloaded plugin, the built-in one:

- Kills a command that times out or is cancelled instead of leaving it running.
- Keeps a command's output within ten times the file's size plus 1 MiB. More than that can't be the formatted file, so it fails the file right away rather than being kept in memory, and the command is killed. Of what a command writes to stderr, the last 64 KiB are kept for the error when it fails.
- Supports a `"setupTimeout"` option (default: `300` seconds): how long a file waits for its `setupCommand`, which is killed once no file waits for it.
- Runs a `setupCommand` once for the files that need it, also when formatting them in parallel. Each file waits for it until its own `"setupTimeout"` or until its formatting is cancelled, while it keeps running for the files that still wait. Its success applies to every file after it. A failure (it couldn't start, it exited unsuccessfully, or the last file waiting for it timed out) applies to every file for 10 seconds rather than being retried for each one, after which the next file runs it again, waiting twice as long after each failure in a row. So a long running process such as an editor's language server recovers from a failure that was transient. After 5 failures in a row (about two and a half minutes), the failure is final until the plugin restarts. A file whose formatting is cancelled doesn't count as a failure.
- On Windows, finds commands through the `PATHEXT` extensions like the Windows shell does, so `.cmd` and `.bat` commands (ex. ones installed with `npm install -g`) work.
- Passes `{{file_path}}`, `{{file_path_with_ext}}`, `{{ext}}`, `{{line_width}}`, `{{use_tabs}}`, `{{indent_width}}`, `{{cwd}}` and `{{timeout}}` to commands as they are, without HTML escaping. They may also be written `{{ file_path }}` or `{{{file_path}}}`, and `\{{` is a literal `{{`. Anything else in `{{` and `}}` (ex. `{{filePath}}`) is reported as a configuration error, rather than failing each file.
- Has its configuration schema built in, so [`dprint schema`](/config#schema) and the language server describe all of the options above without downloading it.
- Takes a `"command"` and `"setupCommand"` as an array or an object too (see [Command forms](#command-forms)).
- Formats the files a command's `"associations"` glob matches, relative to the configuration file's directory like dprint's own `"associations"`, after the formatter that owns them (ex. markdown for `.md`), or alone when none does. The plugin also needed the glob in its own `"associations"`, which takes the files from their formatter.

The built-in exec is part of the CLI, not a release of this plugin. It's named `exec` (ex. in `dprint help` and in configuration diagnostics), has the CLI's version, and is updated by upgrading the CLI: `dprint config update` and `dprint add exec` leave a reference it serves as it is. When an upgrade changes how it formats a file, the incremental cache forgets the files it formatted, and only then.

### Command forms

`"command"` and `"setupCommand"` each take one of three forms:

```toml
# a command line: parts are separated by spaces, and a part in double quotes may contain spaces
[[exec.commands]]
command = "rustfmt --emit stdout"
exts    = ["rs"]

# the program and its arguments
[[exec.commands]]
command = ["gofmt", "-s"]
exts    = ["go"]

# the program, its arguments and environment variables
[[exec.commands]]
command = { executable = "/opt/My Tools/taplo", args = ["fmt", "-"], env = { RUST_LOG = "warn" } }
exts    = ["toml"]
```

- An array or `"args"` passes each argument as it is, through no shell. Empty arguments, spaces and quotes stay.
- `"env"` sets variables over the ones dprint runs with. The program is looked up on the `"PATH"` it sets.
- A command's arguments and `"env"` values may use `{{file_path}}` and the other variables. A setup command's arguments and `"env"` values are used as they are.

### Served commands

A command with `"serve": true` starts once and formats every file it matches. This suits tools that are slow to start, such as a .NET or JVM formatter:

```toml
[[exec.commands]]
command = "dotnet tools/roslyn-format/bin/RoslynFormat.dll --serve"
exts    = ["cs"]
serve   = true
workers = 2
```

dprint writes each file to the command's stdin as one line of JSON:

```json
{ "path": "src/Program.cs", "text": "...", "lineWidth": 120, "indentWidth": 4, "useTabs": false }
```

The command answers each line with one line on stdout:

- `{"text": "..."}` with the formatted text.
- `{"error": "..."}` when it can't format the file, ex. for a syntax error. That file fails, and the command goes on with the next one.

The rest of how it runs:

- `"workers"` (default: `1`) sets how many processes of the command format files at once.
- Its arguments and `"env"` values can't use `{{file_path}}`, `{{file_path_with_ext}}`, or `{{ext}}`, as one process formats every file. Each request carries the file's path and settings, and the other variables are available.
- `"timeout"` applies to each answer. A process that exits, answers with something else, or takes longer fails the file it was formatting and is killed. The next file starts a new process.
- A process that formats no file for 5 minutes ends, and every process ends with dprint.
- Files travel as JSON text, so a file that isn't UTF-8 fails.

### Commands in remote configuration

Commands run programs, so like process plugins and `"includes"`, the exec commands and plugin of a remote configuration file (ex. one `"extends"` refers to by url) are ignored unless a local configuration file allows them with `"playWithFire"` in its exec configuration:

```jsonc
{
  "extends": "https://example.com/dprint.json",
  "exec": {
    // run any program the remote configuration's commands specify
    "playWithFire": true
    // or only commands (and setup commands) that run these programs
    // "playWithFire": ["tombi", "rustfmt"]
  }
}
```

This includes the commands in its `"overrides"`. Its `"cwd"` (the directory commands run in, which decides what a command with a relative path runs, including your local commands) is only used with `"playWithFire": true`. A remote configuration can't allow itself. When none of its commands are allowed, its exec plugin is ignored too.

Remote command checks follow the built-in formatter's host-owned contract for
every exec alias, including versioned and unversioned references. A program
allowlist permits only command-line invocations whose executables can be checked.
Structured invocations (arrays or objects), persistent commands such as `serve`,
and unrecognized command properties require `"playWithFire": true`.

Of the other remote exec properties, only `"lineWidth"`, `"indentWidth"`,
`"useTabs"`, and `"cacheKey"` are accepted without `"playWithFire": true`.
`"timeout"` and `"setupTimeout"` require it so remote commands stay within local
execution limits. Other properties can affect what runs and also require it.

A remote command that sets its own `"cwd"` requires `"playWithFire": true` when
its executable is a relative path, or when a named executable is resolved using
a relative PATH entry. On Windows, batch files (`.cmd` or `.bat`, including npm
shims) require it because they execute through `cmd.exe`. Commands using
`"cacheKeyFiles"` require it because they read local files beyond the permission
to run an allowlisted program.

A nested configuration file with `"inherit": true` inherits the remote commands its ancestor allowed, unless it specifies `"playWithFire"` itself: then it only inherits the ones that allows, and what its ancestor's local configuration files specified in their place (ex. their own `"commands"` or `"cwd"`).

The former `DPRINT_BUILTIN_EXEC` switch has been removed. Setting it to `0` has no
effect. Local or self-hosted manifests identifying the external exec plugin are
rejected; replace those entries with `"exec"` and keep the `exec` configuration.

A manifest or cached identity naming `dprint-plugin-exec` is rejected before its
executable starts. If a differently named manifest starts a plugin that reports
that name at runtime, rejection happens after startup and the metadata exchange.
Startup side effects can already have occurred. Treat every process plugin as
executable code and apply the usual configuration trust requirements; the
identity check cannot establish what an unknown executable will do before it runs.

## Install, Setup, and Configuration

```shellsession
dprint add exec
# legacy alias, also handled by the built-in formatter
dprint add npm:@dprint/exec
```

Configure commands under the `exec` key. The configuration schema describes the host formatter's supported options.

### File paths and matching

`exts` and `fileNames` match case-insensitively, including Unicode letters. Command associations match the real path relative to the configuration directory.

- `{{file_path}}` is the real file path. A shebang-derived extension never changes this value.
- `{{file_path_with_ext}}` is the path passed to parsers. For an extensionless `build` script mapped to `sh`, it ends in `build.dprint-ext-added.sh`.
- `{{ext}}` is the parser-facing extension without its dot, or an empty string when there is no extension. Its case is preserved.

For ordinary files both path variables have the same value. A real filename containing `.dprint-ext-added.` is preserved. Served commands receive the real path in each request. Exec formats whole files and reports that range formatting is unsupported.
