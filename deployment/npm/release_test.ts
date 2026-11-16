import { deepStrictEqual, throws } from "node:assert/strict";
import { createHash } from "node:crypto";
import { expectedArchives, validateArchives, validateUploaded } from "./release.ts";

Deno.test("every declared npm target requires its binary, even on partial matrix success", () => {
  validateArchives([...expectedArchives]);
  for (const name of expectedArchives) throws(() => validateArchives(expectedArchives.filter((other) => other !== name)), /Expected exactly one/);
  throws(() => validateArchives([...expectedArchives, expectedArchives[0]]), /Expected exactly one/);
  throws(() => validateArchives([...expectedArchives, "kprint-unknown.zip"]), /Unexpected archives/);
  deepStrictEqual(expectedArchives.length, 16);
});

const sha256 = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");

function uploaded(lines: (archives: { name: string; digest: string }[]) => string[]) {
  const archives = expectedArchives.map((name) => ({ name, digest: sha256(new TextEncoder().encode(name)) }));
  const checksums = new TextEncoder().encode(lines(archives).map((line) => `${line}\n`).join(""));
  const assets = [...archives, { name: "SHASUMS256.txt", digest: sha256(checksums) }]
    .map(({ name, digest }) => ({ name, state: "uploaded", size: 123, digest: `sha256:${digest}` }));
  return { release: { isDraft: true, targetCommitish: "a".repeat(40), assets }, checksums };
}

const listed = (archives: { name: string; digest: string }[]) =>
  archives.toSorted((a, b) => a.name < b.name ? -1 : 1).map(({ name, digest }) => `${digest}  ${name}`);

Deno.test("finalization accepts kprint-only archives and requires complete binaries and checksums for the planned commit", () => {
  const sha = "a".repeat(40);
  const { release, checksums } = uploaded(listed);
  validateUploaded(release, sha, checksums);
  validateUploaded({ ...release, isDraft: false }, sha, checksums); // Finalization may be retried after GitHub accepted it.
  throws(() => validateUploaded(release, "c".repeat(40), checksums), /source commit/);
  for (const { name } of release.assets) {
    throws(() => validateUploaded({ ...release, assets: release.assets.filter((asset) => asset.name !== name) }, sha, checksums), /Expected exactly one/);
  }
  for (const invalid of [{ state: "new" }, { size: 0 }, { digest: "" }]) {
    throws(() => validateUploaded({ ...release, assets: [{ ...release.assets[0], ...invalid }, ...release.assets.slice(1)] }, sha, checksums), /incomplete/);
  }
});

Deno.test("finalization requires SHASUMS256.txt to list the digest of every archive", () => {
  const sha = "a".repeat(40);
  const { release, checksums } = uploaded(listed);
  throws(() => validateUploaded(release, sha, new TextEncoder().encode("other")), /downloaded SHASUMS256\.txt differs/);
  for (
    const lines of [
      (archives: { name: string; digest: string }[]) => listed(archives).slice(1),
      (archives: { name: string; digest: string }[]) => [...listed(archives), `${"0".repeat(64)}  kprint-extra.zip`],
      (archives: { name: string; digest: string }[]) => listed(archives).toReversed(),
      (archives: { name: string; digest: string }[]) => listed(archives).map((line, index) => index === 0 ? `${"0".repeat(64)}${line.slice(64)}` : line),
    ]
  ) {
    const tampered = uploaded(lines);
    throws(() => validateUploaded(tampered.release, sha, tampered.checksums), /doesn't list the digests/);
  }
  const stale = sha256(new TextEncoder().encode("stale"));
  const replaced = { ...release, assets: release.assets.map((asset, index) => index === 0 ? { ...asset, digest: `sha256:${stale}` } : asset) };
  throws(() => validateUploaded(replaced, sha, checksums), /doesn't list the digests/);
});
