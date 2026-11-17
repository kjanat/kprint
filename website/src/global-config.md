---
title: Global Configuration
description: Documentation on global configuration.
layout: layouts/documentation.njk
---

# Global Configuration

You can maintain a global configuration file that applies formatting config when the current folder does not have a local kprint configuration file.

## Initializing a Global Configuration

Create a global configuration file by running:

```sh
kprint init --global
```

This creates a `kprint.jsonc` file in your system's configuration directory. The default location is:

- **Linux/macOS**: `~/.config/dprint/kprint.jsonc` (or `$XDG_CONFIG_HOME/dprint/kprint.jsonc`)
  - Note: On macOS, it will use the config file in `$HOME/Library/Application Support/dprint` if that folder exists.
- **Windows**: `%APPDATA%\dprint\kprint.jsonc`

The CLI reads `kprint.toml`, `kprint.jsonc` or `kprint.json` from that directory, and falls back to `dprint.toml`, `dprint.jsonc` or `dprint.json`.

You can customize the global config directory by setting the `DPRINT_CONFIG_DIR` environment variable.

## Managing the Global Configuration

Add plugins to your global configuration (alternatively use the `-g` alias instead of `--global`):

```sh
kprint add --global typescript
```

Update plugins in your global configuration:

```sh
kprint config update --global
```

Edit your global configuration file:

```sh
kprint config edit --global
```

The editor to use for the global config follows the same rules as [Editing Config via CLI](/config#editing-config-via-cli) (set the `DPRINT_EDITOR` environment variable to customize it)

## Using the Global Configuration

Once setup, the global configuration will be used by default when there's no kprint or dprint configuration file in the current directory tree; however, to prevent accidentally formatting such directories, a prompt is shown when calling `kprint fmt`:

```
> kprint fmt
Warning You're not in a kprint project. Format '/home/david/dev/scratch' anyway? (Y/n) █

Hint: Specify the directory to bypass this prompt in the future (ex. `kprint fmt .`)
```

As the hint states, you can bypass the confirmation prompt by providing the current directory:

```
> kprint fmt .
Formatted 1 file.
```

To format files using only the global configuration and ignore local configuration files use:

```sh
kprint fmt --config-discovery=global
```
