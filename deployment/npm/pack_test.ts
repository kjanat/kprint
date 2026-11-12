import { deepStrictEqual, throws } from "node:assert/strict";
import { orderedName, packedFilename } from "./pack.ts";

const packed = { name: "@kprint/kprint", version: "1.2.3", filename: "kprint-kprint-1.2.3.tgz" };

Deno.test("npm pack's array and package-keyed JSON both name the one tarball", () => {
  deepStrictEqual(packedFilename([packed], "dist/kprint"), packed.filename);
  deepStrictEqual(packedFilename({ [packed.name]: packed }, "dist/kprint"), packed.filename);
});

Deno.test("an npm pack result without exactly one local tarball stops the build", () => {
  for (
    const invalid of [null, "kprint.tgz", [], {}, [packed, packed], [{ ...packed, filename: "../kprint.tgz" }], [{
      ...packed,
      filename: "kprint.zip",
    }], [{ name: packed.name }]]
  ) {
    throws(() => packedFilename(invalid, "dist/kprint"), /Unexpected npm pack result for dist\/kprint/);
  }
});

Deno.test("tarball names sort in publication order past ten packages", () => {
  const names = [10, 2, 0, 17].map((index) => orderedName(index, "kprint.tgz"));
  deepStrictEqual(names.toSorted(), ["00-kprint.tgz", "02-kprint.tgz", "10-kprint.tgz", "17-kprint.tgz"]);
});
