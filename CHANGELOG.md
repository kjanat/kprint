# Changelog

## Unreleased

- A nightly publishes only when its tarballs hold every platform package, `@kprint/kprint` and `@kjanat/fmt`, each once. A retried nightly points the `nightly` dist-tag at the version npm already has.
- When npm skips optional dependencies, a preview's launcher downloads its platform package from the preview's pkg.pr.new URL. It uses curl, or Node's `http` or `https` module when curl is missing.
- The release workflow deletes every `.zip` asset of a reused draft release that the build didn't produce.
- kprint checks a cached local plugin by its digest when the cache entry has no stored change identity. It used to rebuild the plugin.
- kprint reuses a local plugin's digest by device, inode and change time only on filesystems that keep change times. On Linux and Android these are ext2 to ext4, XFS, Btrfs, tmpfs, F2FS, bcachefs and ZFS. On macOS it is APFS. kprint hashes the plugin on every run on any other filesystem or system.
- Before a release publishes, the workflow checks that `SHASUMS256.txt` matches its release asset and lists the digest of every release archive.
- kprint resolves the schemas and `extends` of a user-selected `configFile` from that file's own directory, after following symlinks. The files a plugin looks for by default stay inside the configuration's directory.
- The README and the website's plugin page list the npm packages and kjanat's plugins.
- kprint reports an ordinary error after a Wasm plugin restart as an error for that file. A second critical failure reports both errors.
- A release has one `kprint-<target>.zip` archive per platform, and `SHASUMS256.txt` lists only those. The `dprint-<target>.zip` copies are gone, so an installer or `upgrade` that downloads them finds no archive in new releases.
- npm previews cover Windows x64, Linux x64 and macOS ARM64. The preview's Linux package holds the static musl binary, and the launcher runs it on glibc and musl.
- A failed nightly keeps its registry errors and the inputs a retry needs.
- The docs say that the cache directory's ancestors must stay in place while kprint runs. They also say that kprint rejects a plugin that reports itself as exec only after the plugin starts.
- kprint uses `tree-fucker` 0.1.0 from crates.io.
- kprint limits npm tarball extraction by entry size, total decompressed bytes, entry count and time. It cancels an extraction worker when the caller stops and removes partial staging data.
- Each formatting arena has its own identifier counters. The info, anchor and condition constructors take the arena they belong to. kprint rejects a reference into another arena and supports nested arenas dropped in any order.
- kprint detects a replaced local plugin even when the replacement keeps the size and modification time, process plugin archives included. It reuses a digest when the file's Unix identity and nanosecond change time match, and hashes the file when that information is missing.
- Exec matches extensions and file names without regard to case. It passes the real path of a file it matched by shebang, adds the `{{file_path_with_ext}}` and `{{ext}}` variables, and reports that it can't format ranges. This change invalidates the built-in exec's formatting cache.
- kprint runs the built-in exec for every exec reference it recognizes. It drops the external-exec switch and the version-specific trust exceptions, and rejects external exec manifests from local or self-hosted sources.
- kprint limits how much of a package manager's manifest it reads and rejects a symlink as the automatic cache directory. It picks the cache per environment, and the docs explain how to clear one project's cache.
- A change to a Wasm plugin's contents or to a process plugin's distribution changes the incremental formatting identity, even when the compiled code stays compatible.
- The docs show three ways to keep Markdown shell examples with placeholders unformatted: an embedded-formatting opt-out, a text fence or an ignore comment.

## 0.62.0 (2026-11-12)

- `extends` accepts npm packages. kprint adds `--reload` controls, an `install` command, `--lines` for range formatting on the command line, and configuration discovery under `.config/`.
- `init` remembers preferences, edits plugin options, selects file types, orders plugins and inserts into existing JSON and TOML configurations.
- kprint verifies checksum-pinned remote configurations. Only trusted configuration layers can configure process plugins, and kprint's exec program allowlists still apply.
- Plugins receive the extension a shebang implies, while associations and overrides match the real path. A range selection carries over from one formatting plugin to the next.
- Editor requests get answers while the configuration changes and during cancellation. The language server corrects range edits, UTF-16 positions, unsaved paths and inheritance from ancestor configurations.
- kprint runs sandboxed WASI reactors and supports native exception handling. Plugin metadata still runs under supervised compilation, interpreter-first execution and execution limits.
- kprint ports upstream formatting layout fixes and the arena-based formatting API. Users of the formatting library must pass an arena to `PrintItems` and its helpers. See `docs/overview.md` and the Wasm SDK arena pool.
- kprint reports deprecation notices from npm and from plugin update metadata. Cached npm configuration works offline, and kprint limits the size of advisory metadata and schema downloads.
- Configuration edits keep TOML array comments and effective init defaults. `init` lists each option choice once, keeps nullable values and redraws its prompts when the terminal resizes.
- Shutdown can interrupt a process plugin's stalled liveness probe or replacement handshake.
- A release publishes the npm tarballs that CI tested, with the platform packages before `@kprint/kprint`. A successful `npm publish` counts as the upload. Platform packages include the LICENSE file.

## 0.61.0 (2026-11-12)

- A plugin declares which configuration entries name JSON Schema files, and kprint passes those files and their local references to the plugin. kprint reads them only inside the configuration directory, after following symlinks. A schema change invalidates the incremental cache.
- kprint loads remote plugin configurations and their references, and resolves a remote schema from the URL that declares it.
- kprint passes an absolute source filename when it formats stdin, so relative schema directives resolve. `--stdin` accepts a path without an extension, and shorthands such as `--stdin .txt` still work.
- kprint has its own cache directory, `~/.cache/kprint/cache` on Linux. `KPRINT_CACHE_DIR` overrides it, and `DPRINT_CACHE_DIR` is the fallback. With `WORKERS_CI=1` or `CF_PAGES=1`, the cache is a `.kprint` folder inside the project's package-manager cache.
- kprint refreshes cached plugin metadata.
- Releases, Docker images and npm packages carry plain versions such as `0.61.0`. A nightly is `<version>-nightly.<date>.g<sha>`.
- Beepboop releases the npm packages and the GitHub release.
- kprint updates wasmtime to 50.0.1, sha2 to 0.11.1, zip to 9.0.5, rustls to 0.23.46, serde_json to 1.0.154 and unicode-width to 0.2.3.

## 0.60.6-kjanat (2026-10-31)

- The binary is `kprint`. The installers also install it as `dprint`, and `kprint --version` prints `kprint 0.60.6`.
- Release archives are `kprint-<target>.zip`. The release also attaches the `dprint-<target>.zip` archives for older installers and `upgrade`.
- A Wasm plugin's overrides can't configure process or exec plugins.
- kprint ignores a remote configuration's negated `excludes` and `..` associations, with a warning.
- A plugin's declared configuration files must sit inside the configuration file's directory. kprint rejects absolute paths, `..` and symlinks that lead out of it.
- kprint expands `${configDir}` and `${originConfigDir}` in a plugin's `configFile`. It resolves `configFile` from the configuration file when it formats a file outside that directory.
- `kprint <subcommand> --help` exits with status 0.
- kprint updates rustls-webpki to 0.103.16 and webpki-roots to 1.0.11.
- npm has `@kprint/kprint` and `@kjanat/fmt`. `@kjanat/fmt` runs `kprint fmt` as `fmt` or `kfmt`, with a default remote configuration when the project has none.
- `ghcr.io/kjanat/kprint` has images for linux/amd64, linux/arm64 and linux/ppc64le. ORAS pulls the other platforms' binaries from the same image.

## 0.60.5-kjanat (2026-10-30)

- A plugin can read a configuration file of its own in TOML, JSON with comments or YAML. By default kprint passes the first file the plugin looks for next to the configuration. `configFile` in the plugin's table names another file, with an optional `#` part, or `false` for none. Remote configuration can only set `false`.
- The language server and the incremental cache pick up changes to a plugin's configuration file.
- A plugin declares the files it looks for in its plugin info or through a `get_config_files` export.
- kprint decides when to compile an interpreted Wasm plugin from the CPU time interpreting takes. Time spent waiting for a CPU or for embedded formatting no longer counts.

## 0.60.4-kjanat (2026-10-30)

- In a TOML configuration whose `plugins` array has no trailing comma, `add` puts the comma after the last plugin and keeps a comment after it in place.

## 0.60.3-kjanat (2026-10-30)

- A run waits for a background compile it started before it exits, so the next run formats with native code.

## 0.60.2-kjanat (2026-10-30)

- kprint runs the built-in exec for every reference to the exec plugin, whatever its release or checksum. That covers `npm:@dprint/exec`, `plugins.dprint.dev`, GitHub releases, jsDelivr and unpkg. `"plugins": ["exec"]` names the built-in exec.
- A measured Wasm format rate counts once it covers a second of formatting. A run that chose the interpreter compiles in the background when interpreting the rest would take longer than compiling.
- When exec can't start a formatter, the error names the program and the directory.
- kprint prints an error that many files share once, then lists the other files after formatting.
- The slow-file warning names the embedded code a plugin waits for.

## 0.60.1-kjanat (2026-10-30)

- On Windows, kprint reads a repository's files from git's index through Git for Windows' fsmonitor named pipe when `core.fsmonitor` and the untracked cache are on.
- The compile notice and the `DPRINT_MAX_PLUGIN_COMPILES` error list each plugin's bytes, file count and first files.
- After 10 seconds, the slow-file warning says what the file waits on. When the file finishes, kprint prints how long it took.
- `--log-level debug` shows the `core.ignoreCase` decisions.
- A release bump changes only the version line of `crates/kprint/Cargo.toml`.

## 0.60.0-kjanat (2026-10-30)

- The crates use `kprint` names. The binary is still `dprint`.
- Exec starts and waits for its processes off the formatting thread. On Windows it resumes each process with `NtResumeProcess`. A run over actionlint's 1,900 files went from 5m49s to 19s.
- A scope walks its directory with `tree-fucker` while its plugins load, and kprint resolves nested config files concurrently.
- On Linux, macOS and FreeBSD, kprint reads files from git's index when the index has an fsmonitor token and an untracked cache, and git's fsmonitor daemon runs.
- kprint looks up npm plugin versions on jsDelivr.
- `.gitignore` matching follows git's `core.ignoreCase`. kprint skips untracked files that git ignores only without regard to case, and formats tracked files inside ignored directories.
- `config update` migrates a URL plugin to an npm specifier only when the plugin's own `latest.json` declares the npm package.
- kprint fetches npm plugins from registry.npmjs.org and jsDelivr without auth and stops reading `.npmrc`. The `minimumDependencyAge` config property sets the minimum dependency age, and `--minimum-dependency-age` overrides it.
- kprint stores incremental files under `incremental/v1`.
- The built-in exec is named `exec` and has kprint's version. Its incremental hash uses its own cache revision.
- With `"serve": true`, exec starts a command once and sends it every file as a JSON line. `"workers"` sets how many processes run, default 1. A remote config needs `"playWithFire": true` for either.
- Exec's `command` and `setupCommand` take an array, or an object with `executable`, `args` and `env`. Exec passes those arguments to the program without a shell.
- An exec command's `associations` glob runs it after the formatter that owns the file, or alone when no formatter does.
- A plugin table and its overrides take `embedded`, which maps the file extension of embedded code to the plugins that format it.
- The "Formatting not stable" error names the plugins that kept changing the file.
- kprint checks the pointer and length a Wasm plugin reports against the plugin's memory size before it allocates. A format result other than 0, 1 or 2 is a critical error.
- kprint frees a plugin config when its last owner drops it.
- Homebrew (`kjanat/homebrew-tap`), Scoop (`kjanat/scoop-bucket`) and the AUR carry the release. npm has it as `kprint`, with platform packages under `@kprint/<target>`.
- The install docs build with `cargo install --locked`.

## 0.59.0-kjanat (2026-10-08)

- kprint resolves Wasm plugin metadata and configuration without native compilation.
- kprint formats small jobs with a Wasm interpreter and compiles larger ones to native code, which later runs reuse. `DPRINT_WASM_FORMAT_ENGINE=auto|interpreter|native` controls the choice.
- kprint discovers files before it sets up plugins, so a scope with no matching files skips plugin downloads and setup. kprint reads an extensionless file only to detect a shebang.
- `DPRINT_MAX_PLUGIN_COMPILES` limits compiles per run, default 50. `DPRINT_GLOB_READ_THREADS` is gone.
- Releases include FreeBSD x86_64 binaries. The installer and process-plugin platform resolution support FreeBSD.
- A change to the built-in exec's formatting invalidates its incremental results. Plugin information and configuration updates identify the built-in exec correctly.
- kprint skips Windows `System Volume Information` and scans large directories without failing.
- `upgrade` and downloads follow this fork's releases and keep working after a repository rename.
- WIT files describe Wasm plugin ABI versions 1 to 4.

## 0.58.0-kjanat (2026-10-07)

- kprint compiles Wasm plugins with wasmtime's parallel compilation.
- A supervised worker process sets up each Wasm plugin. kprint kills and retries the worker when it crashes, stalls or runs past its CPU or wall-clock budget. `DPRINT_WASM_COMPILE_WORKER=0` compiles in process, and `DPRINT_WASM_COMPILE_TIMEOUT` sets a compile's total time. kprint restarts a process plugin once when it doesn't report its info within 20 seconds.
- kprint checks unchanged files against the incremental cache in bulk. It skips reading a file whose size and modification time match the cache.
- On Unix, kprint maps compiled Wasm modules from the cache. It loads them in parallel.
- kprint loads only the Wasm plugins that format files, and caches each plugin's resolved configuration next to its compiled module.
- kprint hashes configuration values with their lengths, so two different configurations can't share incremental results.
- The CLI includes dprint-plugin-exec 0.7.3 and serves exec references without a download. `DPRINT_BUILTIN_EXEC=0` runs the process plugin.
- Exec kills commands that time out, get cancelled or hang in setup. `setupTimeout` limits a setup command, default 300 seconds. Exec retries a failing setup command with growing delays, up to 5 times.
- Exec finds commands through `PATHEXT` on Windows and limits how much of a command's output it keeps.
- A remote configuration's exec commands run only when a local configuration sets `"playWithFire": true` or lists the allowed programs.
- Every child process runs in its own process group or job object. kprint kills the group on timeout, cancellation, Ctrl+C, termination and exit.
- kprint reads `dprint.toml` and `.dprint.toml`, TOML files in `extends` and `--config`, and TOML text passed to `--config`. Commands that edit configuration keep a TOML file's comments and layout.
- kprint reports `extends` cycles, reads a file that two files extend once, and names the file a configuration diagnostic comes from.
- `dprint schema` prints one JSON Schema (2020-12) of the configuration file, with each plugin's schema composed in. `add`, `config update` and `init` regenerate a `dprint.schema.json` next to the configuration.
- kprint reads configuration files through the model it generates the schema from. A root property that is neither dprint's nor an object is an error. A number written as a string, such as `lineWidth: "80"`, is a type error.

## 0.57.5-kjanat (2026-09-14)

- `--config` takes the configuration itself as inline JSON, as `-` or no value for stdin, or as a pipe such as a process substitution.
- kprint re-checks a cached remote configuration file after an hour, or after an immutable response's `max-age`. When the check fails, it uses the cached copy.
- Shell completions complete option values. `--excludes`, `--excludes-override`, `--includes-override` and `--plugins` can repeat, and a bare `--config-discovery` uses the default mode.
