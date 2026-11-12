import { deepStrictEqual, rejects, throws } from "node:assert/strict";
import { packedPackage, type PackedPackage, publishVerified, type RegistryPackage } from "./publish.ts";
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
  const result = await publishVerified(pack, () => Promise.resolve(visible), async () => {
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

Deno.test("publication waits for registry visibility and verifies the built tarball", async () => {
  let reads = 0, publishes = 0, waits = 0;
  await publishVerified(pack, () => Promise.resolve(++reads > 2 ? visible : undefined), async () => {
    publishes++;
  }, async () => {
    waits++;
  });
  deepStrictEqual({ reads, publishes, waits }, { reads: 3, publishes: 1, waits: 1 });
});

Deno.test("existing names, versions or tarballs that differ stop the release", async () => {
  for (const changed of [{ name: "@other/tool" }, { version: "9.9.9" }, { dist: { integrity: "sha512-different" } }]) {
    let publishes = 0;
    await rejects(() =>
      publishVerified(pack, () => Promise.resolve({ ...visible, ...changed }), async () => {
        publishes++;
      }), /differ/);
    deepStrictEqual(publishes, 0);
  }
});

Deno.test("registry failures never become a reason to republish an immutable version", async () => {
  let publishes = 0;
  await rejects(() =>
    publishVerified(pack, () => Promise.reject(new Error("HTTP 500")), async () => {
      publishes++;
    }), /HTTP 500/);
  deepStrictEqual(publishes, 0);
});

Deno.test("an unobservable publication fails instead of reporting success", async () => {
  await rejects(() => publishVerified(pack, () => Promise.resolve(undefined), () => Promise.resolve(), () => Promise.resolve()), /not visible/);
});
