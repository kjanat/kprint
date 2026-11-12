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
    await Deno.writeTextFile(`${directory}/crates/kprint/Cargo.toml`, "version = \"1.2.3\"\n");
    await Deno.writeTextFile(`${directory}/.github/build-matrix.json`, JSON.stringify(matrix));
    for (
      const [name, script] of Object.entries({
        git: `printf '%s\\n' '${sha}'`,
        gh: "printf \"%s\\n\" \"${TEST_VERIFIED:-true}\"",
        npm: "printf \"%s\\n\" \"${TEST_LAST_NIGHTLY:-}\"",
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

Deno.test("a nightly uploads its tarballs in their numbered order, platform packages first", async () => {
  const step = stepOf("publish-npm", "Publish the nightly to npm").run;
  if (step === undefined) throw new Error("The nightly step runs no script");
  const directory = await Deno.makeTempDir({ prefix: "kprint-nightly-test-" });
  try {
    await Deno.mkdir(`${directory}/bin`);
    await Deno.mkdir(`${directory}/deployment/npm/tarballs`, { recursive: true });
    const tarballs = [
      "01-kprint-kprint-win32-x64-1.2.3.tgz",
      "00-kprint-kprint-freebsd-x64-1.2.3.tgz",
      "17-kjanat-fmt-1.2.3.tgz",
      "16-kprint-kprint-1.2.3.tgz",
    ];
    for (const name of tarballs) await Deno.writeTextFile(`${directory}/deployment/npm/tarballs/${name}`, "");
    await Deno.writeTextFile(`${directory}/bin/npm`, `#!/bin/bash\nprintf '%s\\n' "$*" >> "${directory}/calls"\n`);
    await Deno.chmod(`${directory}/bin/npm`, 0o755);
    const result = await new Deno.Command("bash", {
      args: ["-e", "-c", step],
      cwd: directory,
      clearEnv: true,
      env: { PATH: `${directory}/bin:${Deno.env.get("PATH")}`, NPM_TAG: "nightly" },
      stderr: "piped",
    }).output();
    deepStrictEqual(result.code, 0, new TextDecoder().decode(result.stderr));
    deepStrictEqual((await Deno.readTextFile(`${directory}/calls`)).trim().split("\n"), [
      "publish deployment/npm/tarballs/00-kprint-kprint-freebsd-x64-1.2.3.tgz --provenance --access public --tag nightly",
      "publish deployment/npm/tarballs/01-kprint-kprint-win32-x64-1.2.3.tgz --provenance --access public --tag nightly",
      "publish deployment/npm/tarballs/16-kprint-kprint-1.2.3.tgz --provenance --access public --tag nightly",
      "publish deployment/npm/tarballs/17-kjanat-fmt-1.2.3.tgz --provenance --access public --tag nightly",
    ]);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
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
