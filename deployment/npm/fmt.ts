#!/usr/bin/env node
// `fmt [args]` runs `dprint fmt [args]` with the dprint of the main package.
// Without a configuration (no `--config` and none in the directory or its
// ancestors) it formats with the package's bundled kprint.toml, which extends
// kjanat's shared configuration and allows its remote exec commands.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import process from "node:process";
import { defaultConfigArgs } from "./fmt_config.ts";
import { mainPackage } from "./names.ts";

declare const __filename: string;

const args = process.argv.slice(2);
const configArgs = defaultConfigArgs(args, process.cwd(), homedir(), __filename);
const dprintBin = createRequire(__filename).resolve(`${mainPackage}/bin.cjs`);
const result = spawnSync(process.execPath, [dprintBin, "fmt", ...configArgs, ...args], { stdio: "inherit" });
if (result.error != null) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
