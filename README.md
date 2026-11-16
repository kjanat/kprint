# kprint

[![NPM](https://img.shields.io/npm/v/@kprint/kprint?logo=npm&labelColor=CB3837&color=black)][npm:kprint]
[![GitHub release](https://img.shields.io/github/v/release/kjanat/kprint?logo=github&logoColor=white&label=release&labelColor=181717&color=black)](https://github.com/kjanat/kprint/releases/latest)
[![License: MIT](https://img.shields.io/npm/l/@kprint/kprint?color=blue)](LICENSE)

kprint is a pluggable and configurable code formatter written in Rust, forked
from [dprint](https://github.com/dprint/dprint). It runs dprint's plugins and
has formatters built in, such as `exec`.

## Install

```sh
npm install --global @kprint/kprint
kprint help
```

This installs `kprint`, and `dprint` for the tools and editor extensions that
call that. npm downloads only the binary for your platform. For Homebrew, Scoop,
the AUR, mise, Cargo, Docker and the install scripts, see
[Install](https://dprint.kjanat.dev/install).

## npm packages

| Package                        | Version                                                                                                  | Contents                                                                                                    |
| ------------------------------ | -------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| [`@kprint/kprint`][npm:kprint] | [![NPM](https://img.shields.io/npm/v/@kprint/kprint?logo=npm&labelColor=CB3837&color=black)][npm:kprint] | The `kprint` and `dprint` commands. npm installs the matching `@kprint/kprint-<platform>` package with it.  |
| [`@kjanat/fmt`][npm:fmt]       | [![NPM](https://img.shields.io/npm/v/@kjanat/fmt?logo=npm&labelColor=CB3837&color=black)][npm:fmt]       | `fmt` and `kfmt`, which run `kprint fmt`, with kjanat's configuration when the project has none of its own. |

## Links

- [Overview](https://dprint.kjanat.dev/overview)
- [Getting Started](https://dprint.kjanat.dev/install)
- [CLI configuration and plugin setup](docs/cli-configuration.md)
- [Playground](https://dprint.kjanat.dev/playground)

## Plugins

See <https://dprint.kjanat.dev/plugins/>

### kjanat's plugins

| Plugin                                      | Formats                         | npm                                                                                                                                      |
| ------------------------------------------- | ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| [svg][plugin:svg]                           | SVG                             |                                                                                                                                          |
| [svgo][plugin:svgo]                         | SVG, with SVGO (process plugin) |                                                                                                                                          |
| [shfmt][plugin:shfmt]                       | Shell scripts, with shfmt       |                                                                                                                                          |
| [shuck][plugin:shuck]                       | Shell scripts, with Shuck       |                                                                                                                                          |
| [tombi][plugin:tombi]                       | TOML, with Tombi                |                                                                                                                                          |
| [zig][plugin:zig]                           | Zig                             | [![NPM](https://img.shields.io/npm/v/@kjanat/dprint-zig?logo=npm&labelColor=CB3837&color=black)][npm:zig]                                |
| [tex-fmt][plugin:tex-fmt]                   | TeX, with tex-fmt               |                                                                                                                                          |
| [pwsh][plugin:pwsh]                         | PowerShell                      | [![NPM](https://img.shields.io/npm/v/dprint-plugin-pwsh?logo=npm&labelColor=CB3837&color=black)][npm:pwsh]                               |
| [sortpackagejson][plugin:sortpackagejson]   | Sorts `package.json`            | [![NPM](https://img.shields.io/npm/v/@kjanat/dprint-plugin-sortpackagejson?logo=npm&labelColor=CB3837&color=black)][npm:sortpackagejson] |
| [json-schema-sort][plugin:json-schema-sort] | Sorts JSON Schema files         |                                                                                                                                          |

### Other plugins

- [dprint-plugin-typescript](https://github.com/dprint/dprint-plugin-typescript) - TypeScript/JavaScript code formatter.
- [dprint-plugin-json](https://github.com/dprint/dprint-plugin-json) - JSON/JSONC code formatter.
- [dprint-plugin-markdown](https://github.com/dprint/dprint-plugin-markdown) - Markdown code formatter.
- [dprint-plugin-toml](https://github.com/dprint/dprint-plugin-toml) - TOML code formatter.
- [dprint-plugin-jupyter](https://github.com/dprint/dprint-plugin-jupyter) - Jupyter notebook code block formatter.
- [dprint-plugin-dockerfile](https://github.com/dprint/dprint-plugin-dockerfile) - Dockerfile code formatter.
- [dprint-plugin-biome](https://github.com/dprint/dprint-plugin-biome) - Biome (JS/TS/JSON) wrapper plugin.
- [dprint-plugin-oxc](https://github.com/dprint/dprint-plugin-oxc) - Oxc (JS/TS) wrapper plugin.
- [dprint-plugin-prettier](https://github.com/dprint/dprint-plugin-prettier) - Prettier wrapper plugin.
- [dprint-plugin-roslyn](https://github.com/dprint/dprint-plugin-roslyn) - Roslyn (C#/VB) wrapper plugin.
- [dprint-plugin-mago](https://github.com/dprint/dprint-plugin-mago) - Mago (PHP) wrapper plugin.
- [dprint-plugin-ruff](https://github.com/dprint/dprint-plugin-ruff) - Ruff (Python) wrapper plugin.
- [dprint-plugin-exec](https://github.com/dprint/dprint-plugin-exec) - Formats code with any CLI executable.
- [Malva](https://github.com/g-plane/malva) - CSS, SCSS, Sass, and Less formatter
- [markup_fmt](https://github.com/g-plane/markup_fmt) - HTML, Vue, Svelte, Astro, Angular, Jinja, Twig, Nunjucks, Vento, Mustache and XML formatter.
- [pretty_graphql](https://github.com/g-plane/pretty_graphql) - GraphQL formatter.
- [pretty_yaml](https://github.com/g-plane/pretty_yaml) - YAML formatter.
- [dprint-plugin-gofumpt](https://github.com/jakebailey/dprint-plugin-gofumpt) - Go (gofumpt) formatter.
- [panache](https://github.com/jolars/dprint-plugin-panache) - Quarto, Pandoc, R Markdown, and Markdown formatter.
- [badness](https://github.com/jolars/dprint-plugin-badness) - LaTeX and BibTeX formatter.
- [arity](https://github.com/jolars/dprint-plugin-arity) - R formatter.
- [fatou](https://github.com/jolars/dprint-plugin-fatou) - Julia formatter.
- [dprint-plugin-swift](https://github.com/drluckyspin/dprint-plugin-swift) - Swift (SwiftFormat) formatter.

## Sponsor

kjanat maintains kprint and the plugins under [kjanat's plugins](#kjanats-plugins).
dprint is David Sherret's. If your company uses them, please consider sponsoring
[kjanat](https://github.com/sponsors/kjanat) or [David Sherret](https://dprint.dev/sponsor).

[npm:kprint]: https://npm.im/@kprint/kprint
[npm:fmt]: https://npm.im/@kjanat/fmt
[npm:zig]: https://npm.im/@kjanat/dprint-zig
[npm:pwsh]: https://npm.im/dprint-plugin-pwsh
[npm:sortpackagejson]: https://npm.im/@kjanat/dprint-plugin-sortpackagejson
[plugin:svg]: https://github.com/kjanat/dprint-plugin-svg
[plugin:svgo]: https://github.com/kjanat/dprint-plugin-svgo
[plugin:shfmt]: https://github.com/kjanat/dprint-plugin-shfmt
[plugin:shuck]: https://github.com/kjanat/dprint-plugin-shuck
[plugin:tombi]: https://github.com/kjanat/dprint-plugin-tombi
[plugin:zig]: https://github.com/kjanat/dprint-plugin-zig
[plugin:tex-fmt]: https://github.com/kjanat/dprint-plugin-tex-fmt
[plugin:pwsh]: https://github.com/kjanat/dprint-plugin-pwsh
[plugin:sortpackagejson]: https://github.com/kjanat/dprint-plugin-sortpackagejson
[plugin:json-schema-sort]: https://github.com/kjanat/dprint-plugin-json-schema-sort
