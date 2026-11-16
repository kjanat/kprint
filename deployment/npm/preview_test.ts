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

Deno.test("a duplicated, missing or unreferenced package fails before packing", async () => {
  const cases: [string, Record<string, unknown>, RegExp][] = [
    ["@kprint/linux-copy", { name: "@kprint/linux", version: "1.0.0-preview" }, /Duplicate preview packages: @kprint\/linux/],
    ["kprint", {
      name: "@kprint/kprint",
      version: "1.0.0-preview",
      optionalDependencies: {
        "@kprint/linux": "1.0.0-preview",
        "@kprint/mac": "1.0.0-preview",
        "@kprint/win": "1.0.0-preview",
        "@kprint/bsd": "1.0.0-preview",
      },
    }, /Missing preview packages: @kprint\/bsd/],
    ["@kprint/stale", { name: "@kprint/stale", version: "1.0.0-preview" }, /No preview package depends on @kprint\/stale/],
  ];
  for (const [path, json, error] of cases) {
    await fixture(async (directory, originals) => {
      const file = join(directory, path, "package.json");
      await Deno.mkdir(join(directory, path), { recursive: true });
      const text = JSON.stringify(json) + "\n";
      await Deno.writeTextFile(file, text);
      if (originals.has(file)) originals.set(file, text);
      const p = publisher(sizes);
      await rejects(() => publishPreview(directory, "kjanat/kprint", sha, p.command, 19), error);
      deepStrictEqual(p.packed, []);
    });
  }
});

Deno.test("a failed manifest restore still removes staging and keeps the upload's error", async () => {
  await fixture(async (directory, originals) => {
    const p = publisher(sizes, true);
    const main = join(directory, "kprint", "package.json");
    const command = async (args: string[], cwd: string) => {
      if (args[0] !== "npm") {
        await Deno.remove(main);
        await Deno.mkdir(main);
      }
      return await p.command(args, cwd);
    };
    const failure = await publishPreview(directory, "kjanat/kprint", sha, command, 19).then(() => undefined, (error: unknown) => error);
    if (!(failure instanceof AggregateError)) throw new Error(`Expected an AggregateError, got ${failure}`);
    match(String(failure.errors[0]), /upload failed/);
    deepStrictEqual(failure.errors[1] instanceof Deno.errors.IsADirectory, true, String(failure.errors[1]));
    await rejects(() => Deno.stat(p.packed[0]), Deno.errors.NotFound);
    await Deno.remove(main);
    await Deno.writeTextFile(main, originals.get(main) ?? "");
  });
});

Deno.test({
  name: "the launcher downloads a preview's omitted platform package from its preview URL",
  ignore: Deno.build.os !== "linux" || Deno.build.arch !== "x86_64",
  async fn() {
    const directory = await Deno.makeTempDir({ prefix: "kprint-fallback-test-" });
    const binary = "#!/bin/sh\necho preview\n";
    await Deno.mkdir(join(directory, "source", "package"), { recursive: true });
    await Deno.writeTextFile(join(directory, "source", "package", "kprint"), binary);
    const tarball = join(directory, "platform.tgz");
    const packed = await new Deno.Command("bash", { args: ["-c", "exec tar -czf \"$1\" -C \"$2\" package", "tar", tarball, join(directory, "source")] })
      .output();
    deepStrictEqual(packed.code, 0);
    const requests: string[] = [];
    const server = Deno.serve({ hostname: "127.0.0.1", port: 0, onListen() {} }, async (request) => {
      requests.push(new URL(request.url).pathname);
      return new Response(await Deno.readFile(tarball));
    });
    try {
      const root = join(directory, "root");
      await Deno.mkdir(root);
      const bundled = await new Deno.Command("bash", {
        args: [
          "-c",
          "exec \"$@\"",
          "deno",
          Deno.execPath(),
          "bundle",
          "--platform=deno",
          "--format=cjs",
          `--output=${join(root, "bin.cjs")}`,
          fileURLToPath(new URL("./bin.ts", import.meta.url)),
        ],
      }).output();
      deepStrictEqual(bundled.code, 0, new TextDecoder().decode(bundled.stderr));
      const url = `http://127.0.0.1:${server.addr.port}/kjanat/kprint/@kprint/kprint-linux-x64-musl@${sha}`;
      await Deno.writeTextFile(
        join(root, "package.json"),
        JSON.stringify({ version: "1.0.0-preview", optionalDependencies: { "@kprint/kprint-linux-x64-musl": url } }),
      );
      const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(binary)));
      const hash = Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
      await Deno.writeTextFile(join(root, "hashes.json"), JSON.stringify({ "linux-x64-musl": hash }));
      const node = await new Deno.Command("bash", { args: ["-c", "exec node -p process.execPath"], stdout: "piped" }).output();
      const nodeOnly = join(directory, "node-only");
      await Deno.mkdir(nodeOnly);
      await Deno.symlink(new TextDecoder().decode(node.stdout).trim(), join(nodeOnly, "node"));
      // curl downloads first, and node without it
      for (const path of [Deno.env.get("PATH") ?? "", nodeOnly]) {
        await Deno.remove(join(root, "kprint")).catch(() => {});
        const result = await new Deno.Command("bash", {
          args: ["-c", "export PATH=\"$1\"; exec node \"$2\"", "node", path, join(root, "bin.cjs")],
          clearEnv: true,
          env: { PATH: Deno.env.get("PATH") ?? "" },
          stdout: "piped",
          stderr: "piped",
        }).output();
        deepStrictEqual(result.code, 0, new TextDecoder().decode(result.stderr));
        deepStrictEqual(new TextDecoder().decode(result.stdout), "preview\n");
      }
      deepStrictEqual(requests, Array(2).fill(`/kjanat/kprint/@kprint/kprint-linux-x64-musl@${sha}`));
    } finally {
      await server.shutdown();
      await Deno.remove(directory, { recursive: true });
    }
  },
});
