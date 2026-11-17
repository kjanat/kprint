---
title: Ruff Plugin
description: Documentation on the Ruff code formatting plugin for kprint.
layout: layouts/documentation.njk
---

# Ruff Plugin

Adapter plugin that formats Python code via [Ruff](https://docs.astral.sh/ruff/).

Formats .py and .pyi files.

Note: For formatting .ipynb files, use the [Jupyter plugin](/plugins/jupyter) along with this plugin.

## Install and Setup

In your project's directory with a kprint.json file, run:

```shellsession
kprint add ruff
# or install from npm
kprint add npm:@dprint/ruff
```

This will update your config file to have an entry for the plugin. Then optionally specify a `"ruff"` property to add configuration:

```json
{
  "ruff": {
    // ruff's config goes here
  },
  "plugins": [
    "https://plugins.dprint.dev/ruff-x.x.x.wasm"
  ]
}
```

## Configuration

See [Configuration](/plugins/ruff/config)

## Playground

See [Playground](https://dprint.dev/playground#plugin/ruff)
