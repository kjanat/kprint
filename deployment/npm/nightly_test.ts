import { deepStrictEqual, rejects } from "node:assert/strict";
import { createHash } from "node:crypto";
import { basename } from "node:path";
import { publishNightly, registryValue } from "./nightly.ts";

const missing = { code: 1, stdout: "{\"error\":{\"code\":\"E404\"}}", stderr: "not found" };
const ok = { code: 0, stdout: "", stderr: "" };
const failed = { code: 1, stdout: "", stderr: "upload response lost" };
const value = (text: string) => ({ ...ok, stdout: JSON.stringify(text) });
const packages = ["00-first", "01-second"];
const metadata = (path: string) => ({ ...ok, stdout: JSON.stringify({ name: basename(path, ".tgz"), version: "1" }) });

async function fixture(test: (directory: string, integrity: string, report: string) => Promise<void>) {
  const directory = await Deno.makeTempDir();
  try {
    const bytes = new TextEncoder().encode("the original tested tarball");
    await Deno.writeFile(`${directory}/00-first.tgz`, bytes);
    await Deno.writeFile(`${directory}/01-second.tgz`, bytes);
    await test(directory, `sha512-${createHash("sha512").update(bytes).digest("base64")}`, `${directory}/receipts.json`);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

Deno.test("only a structured missing-version error permits publication", async () => {
  deepStrictEqual(await registryValue("package@1", "dist.integrity", () => Promise.resolve(missing)), undefined);
  for (const result of [failed, ok, { ...missing, stdout: "{\"error\":{\"code\":\"E503\"}}" }, { ...ok, stdout: "null" }]) {
    await rejects(() => registryValue("package@1", "dist.integrity", () => Promise.resolve(result)));
  }
});

Deno.test("an ambiguous upload reconciles later integrity and continues to the next tarball", async () => {
  await fixture(async (directory, integrity, report) => {
    let lookups = 0;
    let uploads = 0;
    const delays: number[] = [];
    const tags: string[] = [];
    const receipts = await publishNightly(directory, "nightly", {
      report,
      packages,
      wait: (delay) => {
        delays.push(delay);
        return Promise.resolve();
      },
      command: (_command, args) => {
        if (args[0] === "-xzOf") return Promise.resolve(metadata(args[1]));
        if (args[0] === "view") return Promise.resolve(args[1].includes("01-second") || ++lookups <= 3 ? missing : value(integrity));
        if (args[0] === "dist-tag") {
          tags.push(args.slice(2).join(" "));
          return Promise.resolve(ok);
        }
        uploads++;
        return Promise.resolve(uploads === 1 ? failed : ok);
      },
    });
    deepStrictEqual(uploads, 2);
    deepStrictEqual(delays, [2_000, 5_000]);
    deepStrictEqual(receipts.length, 2);
    deepStrictEqual(receipts.map((receipt) => receipt.outcome), ["available", "accepted"]);
    deepStrictEqual(tags, ["00-first@1 nightly"]);
    deepStrictEqual(JSON.parse(await Deno.readTextFile(report)).packages, receipts);
  });
});

Deno.test("arbitrary upload errors stay failures when no matching integrity appears", async () => {
  await fixture(async (directory, _integrity, report) => {
    let uploads = 0;
    await rejects(() =>
      publishNightly(directory, "nightly", {
        report,
        packages,
        wait: () => Promise.resolve(),
        command: (_command, args) => {
          if (args[0] === "-xzOf") return Promise.resolve(metadata(args[1]));
          if (args[0] === "view") return Promise.resolve(missing);
          uploads++;
          return Promise.resolve(failed);
        },
      }), /npm publish failed/);
    deepStrictEqual(uploads, 1);
    await rejects(() => Deno.readTextFile(report), Deno.errors.NotFound);
  });
});

Deno.test("no receipt is written before the upload promise completes", async () => {
  await fixture(async (directory, _integrity, report) => {
    await Deno.remove(`${directory}/01-second.tgz`);
    const upload = Promise.withResolvers<typeof ok>();
    const started = Promise.withResolvers<void>();
    const publication = publishNightly(directory, "nightly", {
      report,
      packages: ["00-first"],
      command: (_command, args) => {
        if (args[0] === "-xzOf") return Promise.resolve(metadata(args[1]));
        if (args[0] === "view") return Promise.resolve(missing);
        started.resolve();
        return upload.promise;
      },
    });
    await started.promise;
    await rejects(() => Deno.readTextFile(report), Deno.errors.NotFound);
    upload.resolve(ok);
    const receipts = await publication;
    deepStrictEqual(receipts[0].outcome, "accepted");
    deepStrictEqual(JSON.parse(await Deno.readTextFile(report)).packages, receipts);
  });
});

Deno.test("expired tarballs stop recovery before any npm command", async () => {
  const directory = await Deno.makeTempDir();
  try {
    let calls = 0;
    const report = `${directory}/receipts.json`;
    const previous = JSON.stringify({ schemaVersion: 1, packages: [{ spec: "first@1", integrity: "sha512-original", outcome: "accepted" }] });
    await Deno.writeTextFile(report, previous);
    await rejects(() =>
      publishNightly(directory, "nightly", {
        report,
        command: () => {
          calls++;
          return Promise.resolve(ok);
        },
      }), /recover the original artifact/);
    deepStrictEqual(calls, 0);
    deepStrictEqual(await Deno.readTextFile(report), previous);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("a different immutable tarball never reconciles as accepted", async () => {
  await fixture(async (directory, _integrity, report) => {
    let lookups = 0;
    await rejects(() =>
      publishNightly(directory, "nightly", {
        report,
        packages,
        command: (_command, args) => {
          if (args[0] === "-xzOf") return Promise.resolve(metadata(args[1]));
          if (args[0] === "view") return Promise.resolve(lookups++ === 0 ? missing : value("sha512-other"));
          return Promise.resolve(failed);
        },
      }), /npm publish failed for 00-first@1: upload response lost\nnpm has a different 00-first@1/);
  });
});

Deno.test("a missing, extra or duplicated package stops publication before any npm command", async () => {
  await fixture(async (directory, _integrity, report) => {
    for (const expected of [[...packages, "02-third"], ["00-first"], ["00-first", "00-first"]]) {
      const commands: string[] = [];
      await rejects(() =>
        publishNightly(directory, "nightly", {
          report,
          packages: expected,
          command: (command, args) => {
            commands.push(command);
            return Promise.resolve(metadata(args[1]));
          },
        }), /a nightly publishes/);
      deepStrictEqual(commands, ["tar", "tar"]);
    }
  });
});

Deno.test("a failed lookup after an ambiguous upload keeps the upload's error", async () => {
  await fixture(async (directory, _integrity, report) => {
    let lookups = 0;
    await rejects(() =>
      publishNightly(directory, "nightly", {
        report,
        packages,
        command: (_command, args) => {
          if (args[0] === "-xzOf") return Promise.resolve(metadata(args[1]));
          if (args[0] === "view") return Promise.resolve(lookups++ === 0 ? missing : { ...failed, stderr: "registry unavailable" });
          return Promise.resolve(failed);
        },
      }), /npm publish failed for 00-first@1: upload response lost\nnpm lookup failed for 00-first@1: registry unavailable/);
  });
});
