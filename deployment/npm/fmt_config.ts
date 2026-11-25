import { dirname, join } from "node:path";
import { hasConfigFile } from "./config_files.ts";

/** Remote configuration the bundled default extends. */
export const defaultRemoteConfigUrl = "https://raw.githubusercontent.com/kjanat/kjanat/refs/heads/master/configs/dprint.remote.json";

/** File name of the default configuration shipped next to fmt.cjs. */
export const bundledConfigFileName = "kprint.toml";

/** Path to the bundled kprint.toml beside the wrapper script (fmt.ts or fmt.cjs). */
export function bundledConfigPath(fromFile: string): string {
  return join(dirname(fromFile), bundledConfigFileName);
}

export function specifiesConfig(args: string[]): boolean {
  return args.some((arg) => arg === "-c" || arg === "--config" || arg.startsWith("--config="));
}

/**
 * `--config` for `kprint fmt` when the project has no configuration of its
 * own: the package's local kprint.toml, which extends the shared remote
 * configuration and sets exec.playWithFire so those commands can run.
 */
export function defaultConfigArgs(args: string[], cwd: string, home: string | undefined, fromFile: string): string[] {
  return specifiesConfig(args) || hasConfigFile(cwd, home) ? [] : ["--config", bundledConfigPath(fromFile)];
}
