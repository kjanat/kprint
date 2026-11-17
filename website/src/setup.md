---
title: Setup
description: Documentation on setting up kprint to format a collection of code.
layout: layouts/documentation.njk
---

# Setup kprint

After [installing](/install), the main part of getting setup is to create a _kprint.json_/_kprint.jsonc_, or hidden _.kprint.json_/_.kprint.jsonc_ file in your project. The CLI looks for, in order, _kprint.toml_, _.kprint.toml_, _kprint.json_, _kprint.jsonc_, _.kprint.json_ and _.kprint.jsonc_, then the same six names with _dprint_ in place of _kprint_. In a _.config_ directory it looks for _kprint.toml_, _kprint.json_ and _kprint.jsonc_, then the _dprint_ ones. A TOML file wins over a JSON file.

This file will outline:

1. The plugins to use.
2. The configuration to use for formatting files.
3. Which files to include and exclude from formatting.

## Quick Setup

Using the `kprint init` command is a quick way to get setup formatting your project.

Open a terminal in the root directory of your project and run the following command:

```sh
kprint init
```

This will create a _kprint.json_ file in the current working directory. If you are connected to the internet, it will prompt you to select from the latest plugins, pre-selecting the ones that match the files found in the current directory. Use the spacebar to toggle a plugin, type to filter the list, and press enter when finished.

### Non-interactive init

Pass the `--yes` or `-y` flag to skip the prompt and accept the plugins selected based on the files in the current directory. This is useful in scripts:

```sh
kprint init --yes
```

The prompt is also skipped automatically when there is no interactive terminal.

## Manual Setup

Create a _kprint.json_/_kprint.jsonc_ or hidden _.kprint.json_/_.kprint.jsonc_ file in the root directory of the project and read the [configuration documentation](/config).

## Hidden Config File

The CLI supports a default hidden configuration at _.kprint.toml_, _.kprint.json_ or _.kprint.jsonc_, and at _.dprint.toml_, _.dprint.json_ or _.dprint.jsonc_.

## Custom Config File Location

It is recommended to use an auto-discoverable configuration file name (ex. _kprint.json_) as the location of your configuration file because it will be automatically picked up by the CLI and editor plugins. If you place it in another other location then it will need to be manually specified using the `--config <path>` or `-c <path>` flag whenever you run a command.

### `kprint init` with custom config file location

You may specify a custom path for the creation of a configuration file via `kprint init` by specifying it with the `-c` or `--config` flag.

```sh
kprint init --config .kprint.jsonc
kprint init --config path/to/kprint.json
```

## Global Config File

See [global configuration](/global-config)

## Custom Cache Directory

kprint keeps compiled plugins and incremental formatting data in its own cache:

- Linux: `$XDG_CACHE_HOME/kprint/cache`, or `~/.cache/kprint/cache` by default.
- macOS: `~/Library/Caches/kprint/cache`.
- Windows: `%LOCALAPPDATA%/kprint/cache`.

Set `KPRINT_CACHE_DIR` to an absolute path to override the location.
`DPRINT_CACHE_DIR` remains a compatibility fallback when `KPRINT_CACHE_DIR` is
unset or blank. An explicit override takes precedence over Cloudflare detection.
`kprint clear-cache` removes the selected directory, so use a dedicated directory.
The default cache is separate from dprint; existing dprint caches are left in place.
An explicit `DPRINT_CACHE_DIR` or `KPRINT_CACHE_DIR` can intentionally select a
shared or symlinked directory; `clear-cache` then removes that selected target.
Automatically selected cache directories must be real directories. Their parent
may be relocated through a symlink, but the final `cache` or `.kprint` directory
may not itself be a symlink.

This check runs when an environment is created. Later cache reads, writes, and
`clear-cache` use the selected path and require its directory hierarchy to stay
in place for the lifetime of that environment. Renaming or replacing an ancestor
can redirect those operations to a different directory. Keep the cache and its
ancestors under trusted control; stop running kprint processes before relocating
them. Per-environment selection preserves the chosen path, not ownership of the
directory after filesystem changes.

### Cloudflare builds

When `WORKERS_CI=1` or `CF_PAGES=1`, kprint places its cache in a dedicated
`.kprint` subdirectory of the project's package-manager cache:

| Package manager | kprint cache                   |
| --------------- | ------------------------------ |
| npm             | `~/.npm/.kprint`               |
| Yarn            | `~/.cache/yarn/.kprint`        |
| pnpm            | `~/.pnpm-store/.kprint`        |
| Bun             | `~/.bun/install/cache/.kprint` |

kprint looks for `packageManager` in `package.json` or a lockfile, starting in the
current directory and searching parent directories. If neither identifies a
package manager, it uses `npm_config_user_agent`, then defaults to npm.
Within each directory, a recognized `packageManager` takes precedence; lockfile
precedence is Bun, pnpm, Yarn, then npm. Invalid, non-regular, or larger-than-1-MiB
`package.json` files are skipped.

Cache selection happens when each environment is created and stays fixed for
that environment, including its clones. A new environment selects its own cache.
Run `kprint clear-cache` with the same project directory and Cloudflare flags as
the formatting run, or set `KPRINT_CACHE_DIR` to that cache's absolute path.
Running it from another project can select a different package-manager cache;
it only clears the selected directory.

These parent directories are listed in the [Workers Builds cache documentation](https://developers.cloudflare.com/workers/ci-cd/builds/build-caching/)
and [Pages cache documentation](https://developers.cloudflare.com/pages/configuration/build-caching/).
Enable build caching in the Cloudflare project's settings for reuse between
builds. Cloudflare controls cache detection, retention and eviction. kprint
only clears its own `.kprint` subdirectory, preserving package-manager data.

## Proxy

You may specify a proxy for kprint to use when downloading plugins or configuration files by setting the `HTTPS_PROXY`/`https_proxy` and `HTTP_PROXY`/`http_proxy` environment variables.

Additionally, the `NO_PROXY`/`no_proxy` environment variable can be set, which is a comma-separated list of hosts which should not use the proxy.

## TLS Certificates

kprint downloads plugins via HTTPS. In some cases you may wish to configure this. This is possible via the following environment variables:

- `DPRINT_CERT` - Load certificate authority from PEM encoded file.
- `DPRINT_TLS_CA_STORE` - Comma-separated list of order dependent certificate stores.
  - Possible values: `mozilla` and `system`
  - Defaults to `mozilla,system`

### Unsafely ignoring certificates

You can unsafely ignore all or some TLS certificates via the `DPRINT_IGNORE_CERTS` environment variable:

- `DPRINT_IGNORE_CERTS=1` - Ignore all TLS certificates.
- `DPRINT_IGNORE_CERTS=dprint.dev,localhost,[::],127.0.0.1` - Ignore certs from the specified hosts.

This is very unsafe to do and not recommended. A warning will be displayed on first download when this is done.

## Limiting Parallelism

By default, kprint only runs for a short period of time and so it will try to take advantage of as many CPU cores as it can. This might be an issue in some scenarios, and so you can limit the amount of parallelism by setting the `DPRINT_MAX_THREADS` environment variable (ex. `DPRINT_MAX_THREADS=4`).

Finding the files to format doesn't use these threads. kprint scans directories with [tree-fucker](https://github.com/kjanat/tree-fucker), which limits how much file system work runs at once across the whole process.

## Compiling Wasm Plugins

kprint runs a Wasm plugin in an interpreter until compiling it to native code pays off. Getting its plugin info, resolving its configuration, finding the files it formats and its configuration diagnostics always run in the interpreter. They take milliseconds, so commands that don't format, like `kprint output-file-paths` and `kprint config update`, never compile a plugin.

Before formatting, kprint adds up the size of the files each plugin will format, leaving out the files the incremental cache knows are formatted. A plugin is compiled when interpreting those bytes would take longer than compiling it, and formats in the interpreter otherwise. So formatting a few files never waits for a compile. kprint keeps how fast each plugin formatted in the interpreter, so later runs choose with the plugin's own speed. Once a plugin is compiled, its native code is cached and always used.

kprint prints which plugins it compiles before it formats. When that's more than 50, it stops before compiling any, and lists them. Set `DPRINT_MAX_PLUGIN_COMPILES` to allow more (ex. `DPRINT_MAX_PLUGIN_COMPILES=100`). To always interpret or always compile, set `DPRINT_WASM_FORMAT_ENGINE` to `interpreter` or `native`.

The editor integrations (`kprint lsp` and the editor service) format in the interpreter at first. Once a plugin has spent as long in the interpreter as compiling it would take, it compiles in the background and formats natively when that's done.

## Stalled Plugin Setup

kprint compiles a Wasm plugin in a separate process that it watches while it works. When that process crashes, stops making progress (its CPU time stops increasing for 5 seconds, which is also what it looks like when it gets no CPU time at all), or spends far longer on a step than the step needs, kprint kills it and tries again, up to three times. A compile that keeps the CPU busy for too long, or that fails twice, is retried without optimizations. That avoids slow paths in the optimizer, but the plugin may format more slowly until you run `kprint clear-cache`. Together these processes use at most `DPRINT_MAX_THREADS` threads, and each one exits as soon as the kprint process that started it does.

Each attempt also has a time limit however much CPU time it gets, of twice the CPU time all its steps may use, and the compile as a whole (waiting for a free compile process, the attempts and the retries) has twice that, after which kprint gives up on the plugin. The limits grow with the plugin's size, up to 10 minutes in all: the largest plugins compile in about 13 seconds on one thread, so a compile that takes longer than that is stuck rather than large. If a module genuinely needs longer, set `DPRINT_WASM_COMPILE_TIMEOUT` to the number of seconds it may take in all (ex. `DPRINT_WASM_COMPILE_TIMEOUT=1800`), which also works to give it less. A compile is stopped as soon as nothing waits for it anymore.

When kprint can't start that process (ex. it can't find its own executable, or the operating system refuses to [contain](#child-processes) the process), compiling the plugin fails rather than happening in the kprint process, where nothing could stop it.

Process plugins are watched the same way while they start: one that hasn't reported its plugin info within 20 seconds is killed and started once more.

To compile Wasm plugins inside the kprint process instead, without this supervision, set `DPRINT_WASM_COMPILE_WORKER=0`. A compile that hangs then hangs kprint.

## Child Processes

kprint owns the processes it starts (process plugins, Wasm compile processes, and the exec plugin's commands) together with the processes those start, such as the `node` process behind a command installed with `npm install -g`. On Windows each is in a job object, which no process in it can leave. On Linux and macOS each is in a process group of its own, which a process can leave on purpose (`setsid`, ex. a formatter's server started with Node's `detached: true`, which is meant to outlive the command that started it): such a process isn't owned, and outlives everything below.

What's owned is killed once kprint is done with it, when a command times out or formatting is cancelled, and when kprint is interrupted or terminated (ex. Ctrl+C).

When kprint itself is killed (ex. `kill -9`), Windows kills all of them too. On Linux, the processes kprint started directly are killed, except the exec plugin's formatting commands, which usually end on their own once their input and output close. The processes those started can outlive it on Linux and macOS.

Next step: [Configuration](/config)
