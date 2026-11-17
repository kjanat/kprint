import { strictEqual } from "node:assert/strict";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { configFileNames, hasConfigFile } from "./config_files.ts";

function withProject(test: (root: string, nested: string) => void) {
  const root = Deno.makeTempDirSync({ prefix: "kprint-config-files-test-" });
  const nested = join(root, "a", "b");
  mkdirSync(nested, { recursive: true });
  try {
    test(root, nested);
  } finally {
    rmSync(root, { recursive: true });
  }
}

Deno.test("every kprint and dprint name in an ancestor counts as a configuration", () => {
  for (const name of configFileNames) {
    withProject((root, nested) => {
      writeFileSync(join(root, name), "{}");
      strictEqual(hasConfigFile(nested, undefined), true, name);
    });
  }
});

Deno.test("a project's .config directory holds a configuration", () => {
  for (const name of ["kprint.json", "kprint.jsonc", "kprint.toml", "dprint.json", "dprint.jsonc", "dprint.toml"]) {
    withProject((root, nested) => {
      mkdirSync(join(root, ".config"));
      writeFileSync(join(root, ".config", name), "{}");
      strictEqual(hasConfigFile(nested, undefined), true, name);
    });
  }
});

Deno.test("the home directory's .config holds no project configuration", () => {
  withProject((root, nested) => {
    mkdirSync(join(root, ".config"));
    writeFileSync(join(root, ".config", "kprint.json"), "{}");
    strictEqual(hasConfigFile(nested, root), false);
  });
});

Deno.test("a search from a project's .config directory starts at the project", () => {
  withProject((root) => {
    const dotConfig = join(root, ".config");
    mkdirSync(dotConfig);
    writeFileSync(join(dotConfig, ".kprint.json"), "{}");
    strictEqual(hasConfigFile(dotConfig, undefined), false);
    writeFileSync(join(dotConfig, "kprint.json"), "{}");
    strictEqual(hasConfigFile(dotConfig, undefined), true);
  });
});

Deno.test("a search from the home directory's .config starts there", () => {
  withProject((root) => {
    const dotConfig = join(root, ".config");
    mkdirSync(dotConfig);
    writeFileSync(join(dotConfig, ".kprint.json"), "{}");
    strictEqual(hasConfigFile(dotConfig, root), true);
  });
});

Deno.test("a directory without a configuration file has none", () => {
  withProject((_root, nested) => {
    writeFileSync(join(nested, "notkprint.json"), "{}");
    strictEqual(hasConfigFile(nested, undefined), false);
  });
});
