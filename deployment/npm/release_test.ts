import { deepStrictEqual, throws } from "node:assert/strict";
import { expectedArchives, validateArchives, validateUploaded } from "./release.ts";

Deno.test("every declared npm target requires its binary, even on partial matrix success", () => {
  validateArchives([...expectedArchives]);
  for (const name of expectedArchives) throws(() => validateArchives(expectedArchives.filter((other) => other !== name)), /Expected exactly one/);
  throws(() => validateArchives([...expectedArchives, expectedArchives[0]]), /Expected exactly one/);
  throws(() => validateArchives([...expectedArchives, "kprint-unknown.zip"]), /Unexpected archives/);
  deepStrictEqual(expectedArchives.length, 16);
});

Deno.test("finalization accepts kprint-only archives and requires complete binaries and checksums for the planned commit", () => {
  const sha = "a".repeat(40);
  const names = [...expectedArchives, "SHASUMS256.txt"];
  const release = {
    isDraft: true,
    targetCommitish: sha,
    assets: names.map((name) => ({ name, state: "uploaded", size: 123, digest: `sha256:${"b".repeat(64)}` })),
  };
  validateUploaded(release, sha);
  validateUploaded({ ...release, isDraft: false }, sha); // Finalization may be retried after GitHub accepted it.
  throws(() => validateUploaded(release, "c".repeat(40)), /source commit/);
  for (const name of names) {
    throws(() => validateUploaded({ ...release, assets: release.assets.filter((asset) => asset.name !== name) }, sha), /Expected exactly one/);
  }
  for (const invalid of [{ state: "new" }, { size: 0 }, { digest: "" }]) {
    throws(() => validateUploaded({ ...release, assets: [{ ...release.assets[0], ...invalid }, ...release.assets.slice(1)] }, sha), /incomplete/);
  }
});
