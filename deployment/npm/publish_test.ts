import { deepStrictEqual, rejects, throws } from "node:assert/strict";
import { packedPackage, type PackedPackage, publishPackage, type RegistryPackage } from "./publish.ts";
const pack: PackedPackage = { name: "@kprint/kprint", version: "1.2.3", integrity: "sha512-test", filename: "package.tgz" };
const visible: RegistryPackage = { name: pack.name, version: pack.version, dist: { integrity: pack.integrity } };

Deno.test("npm pack accepts both array and package-keyed JSON while checking the intended package", () => {
  deepStrictEqual(packedPackage([pack], pack.name, pack.version), pack);
  deepStrictEqual(packedPackage({ [pack.name]: pack }, pack.name, pack.version), pack);
  for (
    const invalid of [null, [], {}, [pack, pack], [{ ...pack, name: "other" }], [{ ...pack, version: "9.9.9" }], [{ ...pack, filename: "../package.tgz" }]]
  ) {
    throws(() => packedPackage(invalid, pack.name, pack.version), /Unexpected npm pack result/);
  }
});

Deno.test("a retry recognizes an already published facade without publishing it again", async () => {
  let calls = 0;
  const result = await publishPackage(pack, () => Promise.resolve(visible), async () => {
    calls++;
  });
  deepStrictEqual(calls, 0);
  deepStrictEqual(result, {
    id: "pkg:npm/%40kprint/kprint",
    manager: "npm",
    name: pack.name,
    version: pack.version,
    digest: pack.integrity,
    required: true,
    outcome: "succeeded",
  });
});

Deno.test("a successful npm publish reports success without any post-upload registry reads", async () => {
  let reads = 0, publishes = 0;
  const result = await publishPackage(pack, () => {
    reads++;
    if (reads > 1) throw new Error("Post-upload registry reads are forbidden");
    return Promise.resolve(undefined);
  }, async () => {
    publishes++;
  });
  deepStrictEqual({ reads, publishes }, { reads: 1, publishes: 1 });
  deepStrictEqual(result.outcome, "succeeded");
  deepStrictEqual(result.digest, pack.integrity);
});

Deno.test("existing names, versions or tarballs that differ stop the release", async () => {
  for (const changed of [{ name: "@other/tool" }, { version: "9.9.9" }, { dist: { integrity: "sha512-different" } }]) {
    let publishes = 0;
    await rejects(() =>
      publishPackage(pack, () => Promise.resolve({ ...visible, ...changed }), async () => {
        publishes++;
      }), /differ/);
    deepStrictEqual(publishes, 0);
  }
});

Deno.test("registry failures never become a reason to republish an immutable version", async () => {
  let publishes = 0;
  await rejects(() =>
    publishPackage(pack, () => Promise.reject(new Error("HTTP 500")), async () => {
      publishes++;
    }), /HTTP 500/);
  deepStrictEqual(publishes, 0);
});

Deno.test("a failed npm publish propagates its error without post-upload registry reads", async () => {
  let reads = 0;
  await rejects(() =>
    publishPackage(pack, () => {
      reads++;
      return Promise.resolve(undefined);
    }, () => Promise.reject(new Error("npm publish exited with code 1"))), /npm publish exited with code 1/);
  deepStrictEqual(reads, 1);
});
