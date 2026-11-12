import { deepStrictEqual, match } from "node:assert/strict";
import { parse } from "@std/yaml";
import matrix from "../../.github/build-matrix.json" with { type: "json" };

interface Workflow {
  jobs: Record<string, { needs?: string[]; steps: { id?: string; name?: string; run?: string; env?: Record<string, string> }[] }>;
}
const workflow = parse(await Deno.readTextFile(new URL("../../.github/workflows/publish.yml", import.meta.url))) as Workflow;
const plan = workflow.jobs.plan.steps.find((step) => step.id === "plan")!.run!;
const sha = "a".repeat(40);
const context = JSON.stringify({ operation: "promote", version: "1.2.3", sourceSha: sha, draftRelease: { tag: "1.2.3" } });

async function runPlan(env: Record<string, string>) {
  const directory = await Deno.makeTempDir({ prefix: "kprint-plan-test-" });
  try {
    await Deno.mkdir(`${directory}/bin`);
    await Deno.mkdir(`${directory}/crates/kprint`, { recursive: true });
    await Deno.mkdir(`${directory}/.github`);
    await Deno.mkdir(`${directory}/deployment/npm`, { recursive: true });
    await Deno.copyFile(new URL("./nightly.ts", import.meta.url), `${directory}/deployment/npm/nightly.ts`);
    await Deno.writeTextFile(`${directory}/crates/kprint/Cargo.toml`, "version = \"1.2.3\"\n");
    await Deno.writeTextFile(`${directory}/.github/build-matrix.json`, JSON.stringify(matrix));
    for (
      const [name, script] of Object.entries({
        git: `printf '%s\\n' '${sha}'`,
        gh: "printf \"%s\\n\" \"${TEST_VERIFIED:-true}\"",
        npm: `if [[ -n "\${TEST_LOOKUP_ERROR:-}" ]]; then echo '{"error":{"code":"'"$TEST_LOOKUP_ERROR"'"}}'; exit 1; fi
if [[ -n "\${TEST_LAST_NIGHTLY:-}" ]]; then printf '"%s"\\n' "$TEST_LAST_NIGHTLY"; else echo '{"error":{"code":"E404"}}'; exit 1; fi`,
      })
    ) {
      await Deno.writeTextFile(`${directory}/bin/${name}`, `#!/bin/bash\n${script}\n`);
      await Deno.chmod(`${directory}/bin/${name}`, 0o755);
    }
    await Deno.writeTextFile(`${directory}/output`, "");
    const result = await new Deno.Command("bash", {
      args: ["-c", plan],
      cwd: directory,
      clearEnv: true,
      env: {
        PATH: `${directory}/bin:${Deno.env.get("PATH")}`,
        GITHUB_OUTPUT: `${directory}/output`,
        GITHUB_REPOSITORY: "kjanat/kprint",
        GITHUB_EVENT_NAME: "workflow_dispatch",
        RELEASE_CONTEXT: "",
        NIGHTLY: "false",
        DRY_RUN: "false",
        RELEASE_TAG: "",
        PRERELEASE: "false",
        ...env,
      },
      stdout: "piped",
      stderr: "piped",
    }).output();
    return { code: result.code, output: await Deno.readTextFile(`${directory}/output`), error: new TextDecoder().decode(result.stderr) };
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

Deno.test("managed publication plans the complete matrix at the approved signed commit", async () => {
  const result = await runPlan({ RELEASE_CONTEXT: context });
  deepStrictEqual(result.code, 0, result.error);
  match(result.output, /channel=managed\npublish=true\nversion=1.2.3\nnpm_tag=latest/);
  match(result.output, new RegExp(`ref=${sha}`));
  deepStrictEqual(JSON.parse(result.output.split("targets=")[1]), matrix);
});

Deno.test("managed publication rejects unsigned commits, mismatched drafts and incompatible dispatch inputs", async () => {
  const invalidInputs: Record<string, string>[] = [
    { TEST_VERIFIED: "false" },
    { RELEASE_CONTEXT: context.replace("\"tag\":\"1.2.3\"", "\"tag\":\"9.9.9\"") },
    { NIGHTLY: "true" },
    { DRY_RUN: "true" },
  ];
  for (const env of invalidInputs) {
    const result = await runPlan({ RELEASE_CONTEXT: context, ...env });
    deepStrictEqual(result.code, 1);
    deepStrictEqual(result.output, "");
  }
});

Deno.test("published releases only plan downstream distribution", async () => {
  const result = await runPlan({ GITHUB_EVENT_NAME: "release", RELEASE_TAG: "1.2.3" });
  deepStrictEqual(result.code, 0, result.error);
  match(result.output, /channel=distribution/);
  match(result.output, /docker_tags<<EOF\n1.2.3\n1.2\nlatest\nEOF/);
  match(result.output, /targets=\[\]/);
});

Deno.test("nightlies retain their own tag and skip commits already published", async () => {
  const result = await runPlan({ GITHUB_EVENT_NAME: "schedule" });
  deepStrictEqual(result.code, 0, result.error);
  match(result.output, /channel=nightly\npublish=true\nversion=1.2.3-nightly\.[0-9]{8}\.gaaaaaaa\nnpm_tag=nightly/);
  const repeated = await runPlan({ NIGHTLY: "true", TEST_LAST_NIGHTLY: "1.2.3-nightly.20261112.gaaaaaaa" });
  deepStrictEqual(repeated.code, 0, repeated.error);
  match(repeated.output, /publish=false/);
});

Deno.test("stable manual dispatch requires a Beepboop execution", async () => {
  const result = await runPlan({});
  deepStrictEqual(result.code, 1);
  match(result.error, /Start stable releases through Beepboop/);
  deepStrictEqual(result.output, "");
});

function stepOf(job: string, name: string) {
  const step = workflow.jobs[job]?.steps.find((step) => step.name === name);
  if (step === undefined) throw new Error(`${job} has no step ${name}`);
  return step;
}

const nightlyTarballs = [
  ["01-kprint-kprint-win32-x64-1.2.3.tgz", "@kprint/kprint-win32-x64"],
  ["00-kprint-kprint-freebsd-x64-1.2.3.tgz", "@kprint/kprint-freebsd-x64"],
  ["17-kjanat-fmt-1.2.3.tgz", "@kjanat/fmt"],
  ["16-kprint-kprint-1.2.3.tgz", "@kprint/kprint"],
];

async function runNightly(listed: (directory: string) => Promise<Record<string, string>>, lookupError?: string, failSecond = false) {
  const step = stepOf("publish-npm", "Publish the nightly to npm").run;
  if (step === undefined) throw new Error("The nightly step runs no script");
  const directory = await Deno.makeTempDir({ prefix: "kprint-nightly-test-" });
  try {
    const tarballs = `${directory}/deployment/npm/tarballs`;
    await Deno.mkdir(`${directory}/bin`);
    await Deno.mkdir(`${directory}/listed/@kprint`, { recursive: true });
    await Deno.mkdir(`${directory}/listed/@kjanat`);
    await Deno.mkdir(tarballs, { recursive: true });
    await Deno.copyFile(new URL("./nightly.ts", import.meta.url), `${directory}/deployment/npm/nightly.ts`);
    for (const [file, name] of nightlyTarballs) {
      const source = `${directory}/source/${file}/package`;
      await Deno.mkdir(source, { recursive: true });
      await Deno.writeTextFile(`${source}/package.json`, JSON.stringify({ name, version: "1.2.3" }));
      const packed = await new Deno.Command("bash", {
        args: ["-c", "tar -czf \"$1\" -C \"$2\" package", "pack", `${tarballs}/${file}`, `${directory}/source/${file}`],
      }).output();
      deepStrictEqual(packed.code, 0);
    }
    for (const [spec, integrity] of Object.entries(await listed(tarballs))) {
      await Deno.writeTextFile(`${directory}/listed/${spec}`, integrity);
    }
    await Deno.writeTextFile(
      `${directory}/bin/npm`,
      `#!/bin/bash
if [[ "$1" == view ]]; then
  ${lookupError ? `echo '{"error":{"code":"${lookupError}"}}'; exit 1` : ""}
  ${failSecond ? `if [[ "$2" == "@kprint/kprint-win32-x64@1.2.3" && -f "${directory}/failed" ]]; then echo '{"error":{"code":"E503"}}'; exit 1; fi` : ""}
  [[ -f "${directory}/listed/$2" ]] || { echo '{"error":{"code":"E404"}}'; exit 1; }
  printf '"%s"\\n' "$(cat "${directory}/listed/$2")"
  exit 0
fi
printf '%s\\n' "$*" >> "${directory}/calls"
${failSecond ? `if [[ "$2" == *01-* ]]; then touch "${directory}/failed"; echo "npm subprocess failed" >&2; exit 7; fi` : ""}
`,
    );
    await Deno.chmod(`${directory}/bin/npm`, 0o755);
    const result = await new Deno.Command("bash", {
      args: ["-e", "-c", step],
      cwd: directory,
      clearEnv: true,
      env: { PATH: `${directory}/bin:${Deno.env.get("PATH")}`, NPM_TAG: "nightly" },
      stdout: "piped",
      stderr: "piped",
    }).output();
    const calls = await Deno.readTextFile(`${directory}/calls`).catch(() => "");
    return {
      code: result.code,
      receipts: JSON.parse(await Deno.readTextFile(`${directory}/deployment/npm/publication-receipts.json`).catch(() => "null")),
      stdout: new TextDecoder().decode(result.stdout),
      stderr: new TextDecoder().decode(result.stderr),
      calls: calls.trim().split("\n").filter(Boolean),
    };
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

async function integrity(path: string) {
  const digest = await crypto.subtle.digest("SHA-512", await Deno.readFile(path));
  return `sha512-${btoa(String.fromCharCode(...new Uint8Array(digest)))}`;
}

const published = (file: string) => `publish deployment/npm/tarballs/${file} --provenance --access public --tag nightly`;

Deno.test("a nightly uploads its tarballs in their numbered order, platform packages first", async () => {
  const result = await runNightly(() => Promise.resolve({}));
  deepStrictEqual(result.code, 0, result.stderr);
  deepStrictEqual(result.calls, [
    published("00-kprint-kprint-freebsd-x64-1.2.3.tgz"),
    published("01-kprint-kprint-win32-x64-1.2.3.tgz"),
    published("16-kprint-kprint-1.2.3.tgz"),
    published("17-kjanat-fmt-1.2.3.tgz"),
  ]);
});

Deno.test("a retried nightly skips the tarballs npm already lists and uploads the rest", async () => {
  const result = await runNightly(async (tarballs) => ({
    "@kprint/kprint-freebsd-x64@1.2.3": await integrity(`${tarballs}/00-kprint-kprint-freebsd-x64-1.2.3.tgz`),
  }));
  deepStrictEqual(result.code, 0, result.stderr);
  match(result.stdout, /npm already has @kprint\/kprint-freebsd-x64@1\.2\.3/);
  deepStrictEqual(result.calls, [
    published("01-kprint-kprint-win32-x64-1.2.3.tgz"),
    published("16-kprint-kprint-1.2.3.tgz"),
    published("17-kjanat-fmt-1.2.3.tgz"),
  ]);
});

Deno.test("a nightly stops when npm lists a different tarball under the same version", async () => {
  const result = await runNightly(() => Promise.resolve({ "@kprint/kprint-freebsd-x64@1.2.3": "sha512-different" }));
  deepStrictEqual(result.code, 1);
  match(result.stderr, /npm has a different @kprint\/kprint-freebsd-x64@1\.2\.3/);
  deepStrictEqual(result.calls, []);
});

Deno.test("a release publishes its npm tarballs through Beepboop's ledger", () => {
  const publish = stepOf("publish-npm", "Publish to npm");
  match(JSON.stringify(publish), /kjanat\/beepboop\/actions\/publish@[0-9a-f]{40}/);
  match(JSON.stringify(publish), /"npm-tarballs":"deployment\/npm\/tarballs"/);
});

Deno.test("the GitHub release waits only for the binaries", () => {
  deepStrictEqual(workflow.jobs["finalize-release"]?.needs, ["plan", "release-binaries"]);
});

Deno.test("finalization reads the draft release with the Beepboop token", () => {
  const steps = workflow.jobs["finalize-release"]?.steps ?? [];
  const token = steps.findIndex((step) => step.id === "bot");
  const verify = steps.findIndex((step) => step.name === "Verify draft assets before publishing");
  deepStrictEqual(token >= 0 && token < verify, true);
  deepStrictEqual(steps[verify]?.env?.GH_TOKEN, "${{ steps.bot.outputs.token }}");
});

Deno.test("nightly planning stops on registry and authentication failures", async () => {
  for (const error of ["E503", "E401", "E403"]) {
    const result = await runPlan({ NIGHTLY: "true", TEST_LOOKUP_ERROR: error });
    deepStrictEqual(result.code, 1);
    deepStrictEqual(result.output, "");
    match(result.error, /npm lookup failed/);
  }
});

Deno.test("nightly publication never uploads after a failed registry lookup", async () => {
  for (const error of ["E503", "E401", "E403"]) {
    const result = await runNightly(() => Promise.resolve({}), error);
    deepStrictEqual(result.code, 1);
    deepStrictEqual(result.calls, []);
    match(result.stderr, /npm lookup failed/);
  }
});

Deno.test("a failed npm subprocess preserves earlier receipts and fails the job", async () => {
  const result = await runNightly(() => Promise.resolve({}), undefined, true);
  deepStrictEqual(result.code, 1);
  deepStrictEqual(result.calls.length, 2);
  deepStrictEqual(result.receipts.packages.length, 1);
  deepStrictEqual(result.receipts.packages[0].spec, "@kprint/kprint-freebsd-x64@1.2.3");
  deepStrictEqual(result.receipts.packages[0].outcome, "accepted");
});

Deno.test("tested tarballs and partial nightly receipts retain the documented recovery window", () => {
  const artifact = JSON.stringify(stepOf("build-npm", "Upload the npm tarballs"));
  match(artifact, /"retention-days":30/);
  const receipts = JSON.stringify(stepOf("publish-npm", "Preserve nightly publication receipts"));
  match(receipts, /always\(\)/);
  match(receipts, /"retention-days":30/);
  match(receipts, /github.run_attempt/);
});
