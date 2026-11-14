import targets from "./targets.json" with { type: "json" };
import matrix from "../../.github/build-matrix.json" with { type: "json" };

export const expectedArchives = targets.targets.map((target) => target.zipFileName);
export function validateArchives(names: string[]) {
  const built = matrix.map(({ target }) => `kprint-${target}.zip`);
  if (built.length !== expectedArchives.length || !built.every((name) => expectedArchives.includes(name))) {
    throw new Error("Binary build matrix and npm target descriptor disagree");
  }
  for (const name of expectedArchives) {
    if (names.filter((actual) => actual === name).length !== 1) throw new Error(`Expected exactly one archive: ${name}`);
  }
  const unexpected = names.filter((name) => name.startsWith("kprint-") && name.endsWith(".zip") && !expectedArchives.includes(name));
  if (unexpected.length) throw new Error(`Unexpected archives: ${unexpected.join(", ")}`);
}

interface ReleaseAsset {
  name: string;
  state: string;
  size: number;
  digest: string;
}
interface UploadedRelease {
  isDraft: boolean;
  targetCommitish: string;
  assets: ReleaseAsset[];
}
export function validateUploaded(release: UploadedRelease, sha: string) {
  if (!/^[a-f0-9]{40}$/.test(sha) || release.targetCommitish !== sha) throw new Error("Release source commit mismatch");
  validateArchives(release.assets.map((asset) => asset.name));
  const required = [...expectedArchives, "SHASUMS256.txt"];
  for (const name of required) {
    const matches = release.assets.filter((asset) => asset.name === name);
    if (matches.length !== 1) throw new Error(`Expected exactly one uploaded asset: ${name}`);
    const asset = matches[0];
    if (asset.state !== "uploaded" || asset.size <= 0 || !/^sha256:[a-f0-9]{64}$/.test(asset.digest)) {
      throw new Error(`Release asset is incomplete: ${name}`);
    }
  }
}

if (import.meta.main) {
  const [command, path, sha] = Deno.args;
  if (command === "assets" && path) {
    const names: string[] = [];
    for await (const entry of Deno.readDir(path)) {
      if (entry.isFile && (await Deno.stat(`${path}/${entry.name}`)).size > 0) names.push(entry.name);
    }
    validateArchives(names);
    console.log(`Verified all ${expectedArchives.length} binary archives`);
  } else if (command === "uploaded" && path && sha) {
    validateUploaded(JSON.parse(await Deno.readTextFile(path)), sha);
    console.log(`Verified complete release assets for ${sha}`);
  } else throw new Error("Usage: release.ts assets DIRECTORY | uploaded RELEASE_JSON SOURCE_SHA");
}
