import { existsSync } from "node:fs";
import { dirname, join } from "node:path";

// POSSIBLE_CONFIG_FILE_NAMES and config_file_names in kprint-discovery
export const configFileNames = [
  "kprint.toml",
  ".kprint.toml",
  "kprint.json",
  "kprint.jsonc",
  ".kprint.json",
  ".kprint.jsonc",
  "dprint.toml",
  ".dprint.toml",
  "dprint.json",
  "dprint.jsonc",
  ".dprint.json",
  ".dprint.jsonc",
];
const dotConfigFileNames = [
  ".config/kprint.toml",
  ".config/kprint.json",
  ".config/kprint.jsonc",
  ".config/dprint.toml",
  ".config/dprint.json",
  ".config/dprint.jsonc",
];

export function hasConfigFile(dir: string, home: string | undefined): boolean {
  for (let current = dir;; current = dirname(current)) {
    const names = current === home ? configFileNames : [...configFileNames, ...dotConfigFileNames];
    if (names.some((name) => existsSync(join(current, name)))) {
      return true;
    }
    if (dirname(current) === current) {
      return false;
    }
  }
}
