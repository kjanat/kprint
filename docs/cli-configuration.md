# CLI configuration and plugin setup

## Installing and refreshing plugins

`kprint install` downloads and prepares the plugins configured for the current project. It includes inherited plugins when the configuration sets `inherit: true`, and reports whether each plugin was installed or was already available. `kprint config install` is an alias.

```sh
kprint install
kprint install --recursive
kprint install --plugins https://plugins.dprint.dev/typescript-0.95.15.wasm
```

`--recursive` also installs plugins from nested configurations. `--plugins` supplies the plugins for that invocation. Positional plugin names or URLs, as in `kprint install typescript`, first add those plugins to the configuration.

Use `--reload` to refresh downloaded configurations and plugins. `--reload=config` refreshes only configurations, including npm packages in `extends`; `--reload=plugins` reinstalls remote plugins, including versioned npm plugins. Local files and unversioned packages in `node_modules` are read locally. These flags complement automatic remote-configuration freshness checks.

Plugin installation and update commands show publisher deprecation notices from npm metadata and plugin update URLs. Deprecation notices do not change the publisher authorization required to migrate a URL plugin to npm.

## Formatting selected lines

Line numbers are 1-based and ranges include both endpoints:

```sh
kprint fmt src/main.ts --lines 10-20
kprint fmt --lines src/main.ts:10-20,35-40 --lines src/other.ts:5
kprint fmt --stdin main.ts --lines 10-20 < src/main.ts
```

A range without a path requires a single input file or `--stdin`. With `--stdin-files`, put the range after the path on its input line, such as `src/main.ts:10-20`. Overlapping ranges are combined and processed from the end of the file to keep earlier line positions stable. Range formatting bypasses the incremental cache. Plugins decide the formatting extent; plugins without range support may format the whole file.

## Shared configurations from npm

The `extends` property accepts npm packages as well as paths and URLs:

```json
{
  "extends": [
    "npm:@example/format-config",
    "npm:@example/format-config@1.2.3/typescript.json"
  ]
}
```

An unversioned package is resolved from `node_modules` beside the configuration or in an ancestor directory. A versioned package is downloaded from the npm registry. Package exports, JSON `main` entries, and root `dprint.json`, `dprint.jsonc`, or `dprint.toml` files determine the default configuration. Relative references stay inside the package.

Append `@<sha256>` to a versioned reference to pin the package tarball. The checksum covers all files in that package. A trusted package that extends a different remote configuration must pin that reference separately. Tarballs are verified before extraction, and an incorrect checksum is rejected even when the package was already loaded.

Remote process-plugin configuration is accepted only from trusted configuration layers. Local configuration and remote references pinned by a trusted parent can provide that configuration. Unpinned layers can still configure sandboxed WebAssembly plugins. Kprint's existing `playWithFire` program allowlists continue to govern its recognized exec plugin.

An npm registry configuration can use an absolute HTTP(S) `configFile` URL for plugin-owned configuration. It cannot select a local file on the user's machine. Packages resolved from local `node_modules` retain local configuration behavior.

## Project configuration in `.config`

Discovery also checks `.config/dprint.json`, `.config/dprint.jsonc`, and `.config/dprint.toml`. A configuration in the project root takes precedence. The project scope is the parent of `.config`; anchored patterns remain relative to the configuration directory, while floating patterns such as `**/*.ts` match throughout the project. The home directory's `.config` is excluded from project discovery.

`kprint init` creates its configuration inside an existing project `.config` directory and recognizes configurations already stored there. Explicit `--config` and global configuration keep their own locations.

## Interactive initialization

`kprint init` supports choosing plugin options, changing plugin order, and remembering preferred or avoided plugins. Running it against an existing configuration can insert new plugins before existing entries. Existing plugin settings are preserved when adding option blocks. JSON, JSONC, and TOML configurations are supported.

`kprint init --preferences` edits the saved choices. Preferences are stored in `init.json` in the global configuration directory. They include preferred plugin order, excluded file types, and chosen plugin options. Options are loaded when requested, and the prompt can be cancelled while a plugin is loading.
