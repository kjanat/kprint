import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, readdir, rename, writeFile } from "node:fs/promises";
import process from "node:process";
import { join, resolve } from "node:path";
import { setTimeout } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

interface CommandResult {
  code: number;
  stdout: string;
  stderr: string;
}
type Run = (command: string, args: string[]) => Promise<CommandResult>;
const exec = promisify(execFile);
const run: Run = async (command, args) => {
  try {
    const result = await exec(command, args, { encoding: "utf8", maxBuffer: 1024 * 1024, timeout: 120_000 });
    return { code: 0, ...result };
  } catch (error) {
    const failed = error as Error & CommandResult & { killed?: boolean; signal?: string };
    if (typeof failed.code !== "number" && !failed.killed && !failed.signal) throw error;
    return { code: typeof failed.code === "number" ? failed.code : 1, stdout: failed.stdout ?? "", stderr: failed.stderr || failed.message };
  }
};

export async function registryValue(spec: string, field: string, command: Run = run): Promise<string | undefined> {
  const result = await command("npm", ["view", spec, field, "--json"]);
  let value: unknown;
  try {
    value = JSON.parse(result.stdout);
  } catch { /* Report the failed or malformed response below. */ }
  if (result.code !== 0) {
    if (typeof value === "object" && value !== null && "error" in value) {
      const error = value.error;
      if (typeof error === "object" && error !== null && "code" in error && error.code === "E404") return undefined;
    }
    throw new Error(`npm lookup failed for ${spec}: ${result.stderr.trim() || result.stdout.trim() || `exit ${result.code}`}`);
  }
  if (typeof value !== "string" || !value) throw new Error(`npm returned no ${field} for ${spec}`);
  return value;
}

interface Receipt {
  spec: string;
  integrity: string;
  outcome: "accepted" | "available";
}
interface PublishOptions {
  command?: Run;
  wait?: (milliseconds: number) => Promise<unknown>;
  report?: string;
}

export async function publishNightly(directory: string, tag: string, options: PublishOptions = {}): Promise<Receipt[]> {
  const command = options.command ?? run;
  const wait = options.wait ?? setTimeout;
  const report = options.report ?? join(directory, "..", "publication-receipts.json");
  const receipts: Receipt[] = [];
  const files = (await readdir(directory)).filter((name) => name.endsWith(".tgz")).sort();
  if (!files.length) throw new Error("No tested npm tarballs remain; recover the original artifact before retrying publication");
  for (const file of files) {
    const path = join(directory, file);
    const metadata = await command("tar", ["-xzOf", path, "package/package.json"]);
    if (metadata.code !== 0) throw new Error(`Cannot read ${file}: ${metadata.stderr}`);
    const { name, version } = JSON.parse(metadata.stdout);
    if (typeof name !== "string" || typeof version !== "string" || !name || !version) throw new Error(`Invalid package metadata in ${file}`);
    const spec = `${name}@${version}`;
    const integrity = `sha512-${createHash("sha512").update(await readFile(path)).digest("base64")}`;
    const matches = (listed: string | undefined) => {
      if (listed !== undefined && listed !== integrity) throw new Error(`npm has a different ${spec}`);
      return listed === integrity;
    };
    let outcome: Receipt["outcome"] = "available";
    if (matches(await registryValue(spec, "dist.integrity", command))) {
      console.log(`npm already has ${spec}`);
    } else {
      const uploaded = await command("npm", ["publish", path, "--provenance", "--access", "public", "--tag", tag]);
      if (uploaded.code === 0) {
        outcome = "accepted";
      } else {
        // Only ambiguous failures reconcile visibility. Successful uploads do
        // not wait for npm's asynchronous scanning or registry propagation.
        let reconciled = false;
        for (const delay of [0, 2_000, 5_000, 15_000, 30_000]) {
          if (delay) await wait(delay);
          if (matches(await registryValue(spec, "dist.integrity", command))) {
            reconciled = true;
            break;
          }
        }
        if (!reconciled) throw new Error(`npm publish failed for ${spec}: ${uploaded.stderr.trim() || uploaded.stdout.trim() || `exit ${uploaded.code}`}`);
      }
    }
    receipts.push({ spec, integrity, outcome });
    const temporary = `${report}.tmp`;
    await writeFile(temporary, JSON.stringify({ schemaVersion: 1, packages: receipts }, null, 2) + "\n");
    await rename(temporary, report);
  }
  return receipts;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [operation, value, field] = process.argv.slice(2);
  try {
    if (operation === "view" && value && field) console.log(await registryValue(value, field) ?? "");
    else if (operation === "publish" && value && field) await publishNightly(value, field);
    else throw new Error("Usage: nightly.ts view SPEC FIELD | publish DIRECTORY TAG");
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
