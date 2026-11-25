import { deepStrictEqual, match, ok } from "node:assert/strict";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { bundledConfigFileName, bundledConfigPath, defaultConfigArgs, defaultRemoteConfigUrl, specifiesConfig } from "./fmt_config.ts";

const sourceDir = dirname(fileURLToPath(import.meta.url));
const sourceConfig = join(sourceDir, "default-config", bundledConfigFileName);

function withProject(test: (root: string, fromFile: string) => void) {
  const root = Deno.makeTempDirSync({ prefix: "kprint-fmt-config-test-" });
  const fromFile = join(root, "node_modules", "@kjanat", "fmt", "fmt.cjs");
  mkdirSync(dirname(fromFile), { recursive: true });
  writeFileSync(join(dirname(fromFile), bundledConfigFileName), readFileSync(sourceConfig));
  try {
    test(root, fromFile);
  } finally {
    rmSync(root, { recursive: true });
  }
}

Deno.test("the source kprint.toml extends the shared remote configuration and allows remote exec", () => {
  const text = readFileSync(sourceConfig, "utf8");
  match(text, new RegExp(`^extends = "${defaultRemoteConfigUrl.replaceAll("/", "\\/")}"$`, "m"));
  match(text, /^\[exec\]$/m);
  match(text, /^playWithFire = true$/m);
});

Deno.test("the bundled path is the kprint.toml beside the wrapper script", () => {
  deepStrictEqual(
    bundledConfigPath("/tmp/node_modules/@kjanat/fmt/fmt.cjs"),
    join("/tmp/node_modules/@kjanat/fmt", bundledConfigFileName),
  );
});

Deno.test("without a local configuration the wrapper points --config at the bundled file", () => {
  withProject((root, fromFile) => {
    const project = join(root, "project");
    mkdirSync(project);
    deepStrictEqual(defaultConfigArgs([], project, undefined, fromFile), ["--config", bundledConfigPath(fromFile)]);
    deepStrictEqual(defaultConfigArgs(["src/a.rs"], project, undefined, fromFile), ["--config", bundledConfigPath(fromFile)]);
  });
});

Deno.test("a local configuration or an explicit --config leaves the arguments alone", () => {
  withProject((root, fromFile) => {
    const project = join(root, "project");
    mkdirSync(project);
    writeFileSync(join(project, "dprint.json"), "{}");
    deepStrictEqual(defaultConfigArgs([], project, undefined, fromFile), []);
    deepStrictEqual(defaultConfigArgs(["src/a.rs"], project, undefined, fromFile), []);

    const other = join(root, "other");
    mkdirSync(other);
    for (const args of [["--config", "mine.toml"], ["-c", "mine.toml"], ["--config=mine.toml"]]) {
      deepStrictEqual(defaultConfigArgs(args, other, undefined, fromFile), [], String(args));
      ok(specifiesConfig(args), String(args));
    }
    ok(!specifiesConfig(["--config-discovery=ignore"]));
    ok(!specifiesConfig(["src/a.rs"]));
  });
});
