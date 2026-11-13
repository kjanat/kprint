import { join } from "node:path";
import { scope } from "./names.ts";
import { packedFilename } from "./pack.ts";

type Run = (args: string[], cwd: string) => Promise<string>;
const run: Run = async ([command, ...args], cwd) => {
  const result = await new Deno.Command(command, { args, cwd, stdout: "piped", stderr: "inherit" }).output();
  const output = new TextDecoder().decode(result.stdout);
  if (!result.success) throw new Error(`${command} exited with ${result.code}: ${output}`);
  return output;
};

export async function publishPreview(directory: string, repository: string, sha: string, command: Run = run, maxBytes = 95 * 1024 * 1024) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository) || !/^[a-f0-9]{40}$/.test(sha)) throw new Error("Expected a repository and full preview commit SHA");
  directory = await Deno.realPath(directory);
  const platforms: string[] = [];
  for await (const entry of Deno.readDir(join(directory, scope))) {
    if (entry.isDirectory) platforms.push(join(directory, scope, entry.name));
  }
  platforms.sort();
  if (!platforms.length) throw new Error("No preview platform packages");
  const facades = [join(directory, "kprint"), join(directory, "fmt")];
  const manifests = await Promise.all([...platforms, ...facades].map(async (path) => {
    const file = join(path, "package.json");
    const original = await Deno.readTextFile(file);
    return { path, file, original, json: JSON.parse(original) };
  }));
  const urls = new Map(manifests.map(({ json }) => [json.name, `https://pkg.pr.new/${repository}/${json.name}@${sha}`]));
  const temporary = await Deno.makeTempDir({ prefix: "kprint-preview-" });
  try {
    // Prebuilt tarballs are uploaded unchanged by pkg-pr-new.
    for (const { file, json } of manifests) {
      for (const field of ["dependencies", "optionalDependencies"]) {
        for (const name of Object.keys(json[field] ?? {})) {
          const url = urls.get(name);
          if (url) json[field][name] = url;
        }
      }
      await Deno.writeTextFile(file, JSON.stringify(json, null, 2) + "\n");
    }
    const packed: string[] = [];
    let bytes = 0;
    for (const { path } of manifests) {
      const result = JSON.parse(await command(["npm", "pack", "--ignore-scripts", "--json", "--pack-destination", temporary], path));
      const tarball = join(temporary, packedFilename(result, path));
      const size = (await Deno.stat(tarball)).size;
      bytes += size;
      packed.push(tarball);
    }
    if (bytes > maxBytes) {
      throw new Error(
        `Preview tarballs total ${bytes} bytes, exceeding the ${maxBytes}-byte upload budget. Reduce their size or arrange pkg.pr.new multipart access.`,
      );
    }
    // The service consumes the workflow's authorization after one publication.
    console.log(`Publishing ${packed.length} preview packages (${bytes} compressed bytes)`);
    console.log(await command(["npx", "--yes", "pkg-pr-new@0.0.93", "publish", "--no-compact", "--comment=update", "--bin", ...packed], directory));
  } finally {
    await Promise.all(manifests.map(({ file, original }) => Deno.writeTextFile(file, original)));
    await Deno.remove(temporary, { recursive: true });
  }
}

if (import.meta.main) await publishPreview(Deno.args[0], Deno.args[1], Deno.args[2]);
