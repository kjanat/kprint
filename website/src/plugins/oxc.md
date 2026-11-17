---
title: Oxc Plugin
description: Documentation on the Oxc code formatting plugin for kprint.
layout: layouts/documentation.njk
---

# Oxc Plugin

Adapter plugin that formats JavaScript and TypeScript files via [Oxc](https://oxc.rs).

## Install and Setup

In your project's directory with a kprint.json file, run:

```shellsession
kprint add oxc
# or install from npm
kprint add npm:@dprint/oxc
```

This will update your config file to have an entry for the plugin. Then optionally specify a `"oxc"` property to add configuration:

```json
{
  "oxc": {
    // oxc's config goes here
  },
  "plugins": [
    "https://plugins.dprint.dev/oxc-x.x.x.wasm"
  ]
}
```

## Configuration

See [Configuration](/plugins/oxc/config)

## Playground

See [Playground](https://dprint.dev/playground#plugin/oxc)

## Source

See https://github.com/dprint/dprint-plugin-oxc
