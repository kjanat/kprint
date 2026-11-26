# Upstream Markdown formatter

This directory vendors the formatter engine from
[dprint/dprint-plugin-markdown](https://github.com/dprint/dprint-plugin-markdown)
release `0.24.0`, commit `02f8a427e737ba7399a6259683bdd791c32036b9`
(tree `c524abe836686010bc80e122a56af0660ead681b`). The upstream MIT license
and copyright notice are preserved in [vendor/LICENSE](vendor/LICENSE).

The Wasm adapter is omitted. Imports are adjusted for this nested module;
any further integration changes are maintained locally.

The vendored release schema has SHA-256
`2d47bde842033d9b718aef616c0e066de2656f1a7bf6807d4da2532f6c5f9b6e`.
The official release Wasm, the npm package's Wasm, and
`https://plugins.dprint.dev/markdown-0.24.0.wasm` all have SHA-256
`cf7e65674b7eb5d91f85152ae9020dc8b3e73bd7944efcbfb05df71081217764`.
The native integration uses kprint configuration types and adds a parse-once
collect/replay interface for asynchronous embedded formatting.
