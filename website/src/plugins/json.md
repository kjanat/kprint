---
title: JSON Plugin
description: Documentation on the JSON code formatting plugin for kprint.
layout: layouts/documentation.njk
---

# JSON/JSONC Code Formatter

Supports:

- JSON
- JSONC (JSON with comments)

## Install and Setup

In your project's directory with a kprint.json file, run:

```shellsession
kprint add json
# or install from npm
kprint add npm:@dprint/json
```

This will update your config file to have an entry for the plugin. Then optionally specify a `"json"` property to add configuration:

```json
{
  "json": {
    // json config goes here
  },
  "plugins": [
    "https://plugins.dprint.dev/json-x.x.x.wasm"
  ]
}
```

## Configuration

See [Configuration](/plugins/json/config)

## Playground

See [Playground](https://dprint.dev/playground#plugin/json)
