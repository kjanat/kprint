---
title: Jupyter Plugin
description: Documentation on the Jupyter Notebook code block formatting plugin for kprint.
layout: layouts/documentation.njk
---

# Jupyter Plugin

Formats code blocks in Jupyter Notebooks.

## Install and Setup

In your project's directory with a kprint.json file, run:

```shellsession
kprint add jupyter
# or install from npm
kprint add npm:@dprint/jupyter
```

This will update your config file to have an entry for the plugin.

```json
{
  "plugins": [
    "https://plugins.dprint.dev/jupyter-x.x.x.wasm"
  ]
}
```

Then add some additional formatting plugins to format the code blocks with. For example:

```shellsession
kprint add typescript
kprint add markdown
kprint add ruff
```

If you find a code block isn't being formatted with a plugin, please verify it's not a syntax error. After, open an [issue](https://github.com/dprint/dprint-plugin-jupyter/issues) about adding support for that plugin (if you're interested in opening a PR, it's potentially an easy contribution).

## Configuration

Set the configuration for code blocks in other plugins.
