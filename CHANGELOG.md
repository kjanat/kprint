# Changelog

## Unreleased

- Detect local plugin replacements even when size and mtime are preserved, including process archives. Reuse digests when stable Unix file identities and nanosecond change times match; hash sources when that information is unavailable.

- Match exec extensions and filenames case-insensitively. Preserve real paths for shebang-matched files, add `{{file_path_with_ext}}` and `{{ext}}`, and explicitly report that exec does not support range formatting. Invalidate the built-in exec formatting cache for the changed behavior.

- Always use built-in exec for recognized references. Remove the external-exec switch and version-specific trust exceptions; reject local or self-hosted external exec manifests.

- Bound package-manager manifest reads and reject symlinked automatic cache directories. Select the cache per environment and document project-specific cache clearing.
- Include Wasm contents and process-plugin distributions in incremental formatting identity, independently of compiled-code compatibility.
- Document how to preserve Markdown shell examples with placeholders using embedded-formatting opt-outs, text fences, or ignore comments.

- Add npm packages in `extends`, explicit `--reload` controls, an `install` command, CLI range formatting with `--lines`, and project configuration discovery under `.config/`.
- Add init preferences, plugin option editing, file-type selection, plugin ordering, and insertion into existing JSON and TOML configurations.
- Verify checksum-pinned remote configurations and restrict process-plugin configuration to trusted layers while preserving kprint's exec program allowlists.
- Pass shebang-derived extensions to plugins while retaining real paths for associations and overrides. Preserve selections across consecutive formatting plugins.
- Keep editor requests responsive during configuration changes and cancellation. Correct LSP range edits, UTF-16 positions, unsaved paths, and ancestor configuration inheritance.
- Support sandboxed WASI reactors and native exception handling while preserving supervised compilation, interpreter-first execution, and execution limits for plugin metadata.
- Port upstream formatting layout fixes and the arena-based formatting API. Formatting library users must now supply an arena to `PrintItems` and its helpers; see `docs/overview.md` and the Wasm SDK arena pool.
- Report deprecation notices from npm and plugin update metadata. Keep cached npm configuration usable offline and bound advisory metadata and schema downloads.
- Preserve TOML array comments and effective init defaults, deduplicate option choices, retain nullable values, and redraw prompts on terminal resize.
- Allow shutdown to interrupt stalled process-plugin liveness probes and replacement handshakes.
