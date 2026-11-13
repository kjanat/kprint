import { deepStrictEqual, match, rejects } from "node:assert/strict";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
import { publishPreview } from "./preview.ts";

const sha = "a".repeat(40);
const url = (name: string) => `https://pkg.pr.new/kjanat/kprint/${name}@${sha}`;

Deno.test({
  name: "the launcher selects static-only previews on both libcs and preserves full-release selection",
  ignore: Deno.build.os !== "linux" || Deno.build.arch !== "x86_64",
  async fn() {
    const directory = await Deno.makeTempDir({ prefix: "kprint-libc-test-" });
    const exec = async (args: string[]) => {
      const result = await new Deno.Command("bash", { args: ["-c", "exec \"$@\"", "test", ...args], stdout: "piped", stderr: "piped" }).output();
      deepStrictEqual(result.code, 0, new TextDecoder().decode(result.stderr));
      return new TextDecoder().decode(result.stdout).trim();
    };
    try {
      const binary = join(directory, "bin.cjs");
      await exec([Deno.execPath(), "bundle", "--platform=deno", "--format=cjs", `--output=${binary}`, fileURLToPath(new URL("./bin.ts", import.meta.url))]);
      for (const fullRelease of [false, true]) {
        for (const family of ["glibc", "musl"]) {
          const root = join(directory, `${fullRelease}-${family}`);
          await Deno.mkdir(root);
          await Deno.copyFile(binary, join(root, "bin.cjs"));
          const optionalDependencies: Record<string, string> = {};
          for (const target of fullRelease ? ["glibc", "musl"] : ["musl"]) {
            const name = `@kprint/kprint-linux-x64-${target}`;
            optionalDependencies[name] = "1.0.0";
            const path = join(root, "node_modules", name);
            await Deno.mkdir(path, { recursive: true });
            await Deno.writeTextFile(join(path, "package.json"), JSON.stringify({ name, version: "1.0.0" }));
            await Deno.writeTextFile(join(path, "kprint"), `#!/bin/sh\nprintf '%s\\n' '${target}'\n`);
            await Deno.chmod(join(path, "kprint"), 0o755);
          }
          await Deno.writeTextFile(join(root, "package.json"), JSON.stringify({ optionalDependencies }));
          const preload = join(root, "libc.cjs");
          await Deno.writeTextFile(
            preload,
            `const fs=require('node:fs');const read=fs.readFileSync;fs.readFileSync=(p,...args)=>p==='/usr/bin/ldd'?'${family}':read(p,...args);`,
          );
          deepStrictEqual(await exec(["node", "--require", preload, join(root, "bin.cjs")]), fullRelease ? family : "musl");
        }
      }
    } finally {
      await Deno.remove(directory, { recursive: true });
    }
  },
});

async function fixture(test: (directory: string, originals: Map<string, string>) => Promise<void>) {
  const directory = await Deno.makeTempDir({ prefix: "kprint-preview-test-" });
  const originals = new Map<string, string>();
  try {
    for (
      const [path, json] of [
        ["@kprint/linux", { name: "@kprint/linux", version: "1.0.0-preview" }],
        ["@kprint/mac", { name: "@kprint/mac", version: "1.0.0-preview" }],
        ["@kprint/win", { name: "@kprint/win", version: "1.0.0-preview" }],
        ["kprint", {
          name: "@kprint/kprint",
          version: "1.0.0-preview",
          optionalDependencies: { "@kprint/linux": "1.0.0-preview", "@kprint/mac": "1.0.0-preview", "@kprint/win": "1.0.0-preview" },
        }],
        ["fmt", { name: "@kjanat/fmt", version: "1.0.0-preview", dependencies: { "@kprint/kprint": "1.0.0-preview" } }],
      ] as const
    ) {
      const folder = join(directory, path);
      await Deno.mkdir(folder, { recursive: true });
      const file = join(folder, "package.json");
      const text = JSON.stringify(json) + "\n";
      originals.set(file, text);
      await Deno.writeTextFile(file, text);
    }
    await test(directory, originals);
    for (const [file, text] of originals) deepStrictEqual(await Deno.readTextFile(file), text);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

function publisher(sizes: Record<string, number>, failUpload = false) {
  const uploads: string[][] = [];
  const packed: string[] = [];
  const command = async (args: string[], cwd: string) => {
    if (args[0] === "npm") {
      const name = basename(cwd);
      const filename = `${name}.tgz`;
      const file = join(args[args.indexOf("--pack-destination") + 1], filename);
      await Deno.writeFile(file, new Uint8Array(sizes[name]));
      packed.push(file);
      return JSON.stringify([{ filename }]);
    }
    deepStrictEqual(packed.length, 5, "preflight every package before uploading any");
    deepStrictEqual(args.slice(0, 5), ["npx", "--yes", "pkg-pr-new@0.0.93", "publish", "--no-compact"]);
    const paths = args.slice(args.indexOf("--bin") + 1);
    uploads.push(paths.map((path) => basename(path)));
    if (failUpload) throw new Error("upload failed");
    deepStrictEqual(args[5], "--comment=update");
    deepStrictEqual(paths, packed);
    const main = JSON.parse(await Deno.readTextFile(join(cwd, "kprint/package.json")));
    deepStrictEqual(main.optionalDependencies, Object.fromEntries(["linux", "mac", "win"].map((name) => [`@kprint/${name}`, url(`@kprint/${name}`)])));
    const fmt = JSON.parse(await Deno.readTextFile(join(cwd, "fmt/package.json")));
    deepStrictEqual(fmt.dependencies, { "@kprint/kprint": url("@kprint/kprint") });
    return "published";
  };
  return { command, uploads, packed };
}

const sizes = { linux: 6, mac: 6, win: 4, kprint: 2, fmt: 1 };

Deno.test("preview publishes all measured tarballs in one authorized request", async () => {
  await fixture(async (directory) => {
    const p = publisher(sizes);
    await publishPreview(directory, "kjanat/kprint", sha, p.command, 19);
    deepStrictEqual(p.uploads, [["linux.tgz", "mac.tgz", "win.tgz", "kprint.tgz", "fmt.tgz"]]);
    await rejects(() => Deno.stat(p.packed[0]), Deno.errors.NotFound);
  });
});

Deno.test("a failed upload propagates without a second publication and cleans staging", async () => {
  await fixture(async (directory) => {
    const p = publisher(sizes, true);
    await rejects(() => publishPreview(directory, "kjanat/kprint", sha, p.command, 19), /upload failed/);
    deepStrictEqual(p.uploads, [["linux.tgz", "mac.tgz", "win.tgz", "kprint.tgz", "fmt.tgz"]]);
    await rejects(() => Deno.stat(p.packed[0]), Deno.errors.NotFound);
  });
});

Deno.test("oversized individual or aggregate tarballs fail before any upload", async () => {
  for (const overrides of [{ linux: 11 }, { kprint: 6, fmt: 5 }]) {
    await fixture(async (directory) => {
      const p = publisher({ ...sizes, ...overrides });
      await rejects(() => publishPreview(directory, "kjanat/kprint", sha, p.command, 10), /exceed/);
      deepStrictEqual(p.uploads, []);
    });
  }
});

Deno.test("a packing subprocess failure prevents all uploads", async () => {
  await fixture(async (directory) => {
    await rejects(() =>
      publishPreview(directory, "kjanat/kprint", sha, async (args) => {
        deepStrictEqual(args[0], "npm");
        throw new Error("npm pack exited with 7");
      }), /npm pack exited with 7/);
  });
});

Deno.test("real npm tarballs retain commit-specific dependencies when uploaded unchanged", async () => {
  await fixture(async (directory) => {
    const uploads: string[][] = [];
    const command = async (args: string[], cwd: string) => {
      if (args[0] === "npm") {
        const result = await new Deno.Command("bash", { args: ["-c", "exec npm \"$@\"", "npm", ...args.slice(1)], cwd, stdout: "piped", stderr: "piped" })
          .output();
        deepStrictEqual(result.code, 0, new TextDecoder().decode(result.stderr));
        const output = new TextDecoder().decode(result.stdout);
        const [{ filename }] = JSON.parse(output);
        const tarball = join(args[args.indexOf("--pack-destination") + 1], filename);
        const extracted = await new Deno.Command("bash", { args: ["-c", "exec tar -xOf \"$1\" package/package.json", "tar", tarball], stdout: "piped" })
          .output();
        deepStrictEqual(extracted.code, 0);
        const json = JSON.parse(new TextDecoder().decode(extracted.stdout));
        for (const value of Object.values(json.dependencies ?? json.optionalDependencies ?? {})) match(String(value), new RegExp(`@${sha}$`));
        return output;
      }
      uploads.push(args.slice(args.indexOf("--bin") + 1));
      return "published";
    };
    await publishPreview(directory, "kjanat/kprint", sha, command);
    deepStrictEqual(uploads.length, 1);
    deepStrictEqual(uploads[0].length, 5);
  });
});
