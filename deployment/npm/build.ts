#!/usr/bin/env -S deno run -A
import $ from "@david/dax";
import { parseArgs } from "@std/cli/parse-args";
// @ts-types="@types/decompress"
import decompress from "decompress";
import { fmtPackage, mainPackage, platformDir, platformPackage, scope } from "./names.ts";
import { orderedName, packedFilename } from "./pack.ts";
import targets from "./targets.json" with { type: "json" };

interface Package {
  zipFileName: string;
  os: "win32" | "darwin" | "linux" | "android" | "freebsd";
  cpu: "x64" | "arm64" | "riscv64" | "loong64" | "ppc64";
  libc?: "glibc" | "musl";
}

const args = parseArgs(Deno.args, {
  boolean: ["exact-version"],
  // `zips`: a directory with the release zips (ex. a CI run's artifacts) to
  // package instead of the release's. Targets without a zip there aren't
  // packaged. `out`: the directory to build in, instead of `dist`. `pack`: a
  // directory for the packages' tarballs, numbered in publication order.
  string: ["zips", "out", "pack"],
});
const allPackages: Package[] = targets.targets.map((target) => {
  const { os, cpu, libc } = target;
  if (os !== "win32" && os !== "darwin" && os !== "linux" && os !== "android" && os !== "freebsd") {
    throw new Error(`Invalid npm target OS: ${os}`);
  }
  if (cpu !== "x64" && cpu !== "arm64" && cpu !== "riscv64" && cpu !== "loong64" && cpu !== "ppc64") {
    throw new Error(`Invalid npm target CPU: ${cpu}`);
  }
  if (libc !== undefined && libc !== "glibc" && libc !== "musl") {
    throw new Error(`Invalid npm target libc: ${libc}`);
  }
  const pkg = [os, cpu, libc].filter(Boolean).join("-");
  if (target.pkg !== pkg) throw new Error(`Npm target ${target.pkg} must be named ${pkg}`);
  return { zipFileName: target.zipFileName, os, cpu, libc };
});

// fmt.ts's configuration without one
const fmtDefaultConfig = "https://raw.githubusercontent.com/kjanat/kjanat/refs/heads/master/configs/dprint.remote.json";

const mainReadme = (repositoryUrl: string) =>
  `# ${mainPackage}

[kprint](https://dprint.kjanat.dev) is a pluggable and configurable code
formatter written in Rust, forked from dprint. It has formatters built in (ex.
exec) and runs dprint's plugins.

## Install

\`\`\`sh
npm install --global ${mainPackage}
\`\`\`

This installs the \`kprint\` command, also as \`dprint\` for tools
that call that. npm only downloads the binary for your platform, from its
\`${mainPackage}-<platform>\` package.

## Use

\`\`\`sh
kprint init   # create a configuration file
kprint fmt    # format the files
kprint check  # check they're formatted
\`\`\`

Or in a project, as a dev dependency:

\`\`\`sh
npm install --save-dev ${mainPackage}
npx kprint fmt
\`\`\`

[${fmtPackage}](https://www.npmjs.com/package/${fmtPackage}) runs \`kprint fmt\` as \`fmt\`.

See [dprint.kjanat.dev](https://dprint.kjanat.dev) for the configuration and the
plugins, and [the repository](${repositoryUrl}) for its source.
`;

const currentDir = $.path(import.meta.url).parentOrThrow();
const rootDir = currentDir.parentOrThrow().parentOrThrow();
const outputDir = args.out == null ? currentDir.join("./dist") : $.path(args.out).resolve();
const scopeDir = outputDir.join(scope);
const dprintDir = outputDir.join("kprint");
const fmtDir = outputDir.join("fmt");
// the release tag (ex. `0.60.5-kjanat`), which the zips are downloaded from
const version = resolveVersion();
// The npm version: the tag without the fork's `-kjanat`, which tells its
// releases from dprint's but makes every one a prerelease to npm. A nightly or
// preview version (ex. `0.60.6-nightly.20261031.g1234567`) is kept.
const npmVersion = args["exact-version"] ? version : version.replace(/-kjanat$/, "");

const zipsDir = args.zips == null ? undefined : $.path(args.zips);
const packages = zipsDir == null
  ? allPackages
  : allPackages.filter((pkg) => zipsDir.join(pkg.zipFileName).existsSync());
if (packages.length === 0) throw new Error(`No release zips in ${zipsDir}.`);

function isPortableLinuxPackage(pkg: Package): boolean {
  return pkg.os === "linux" && pkg.libc === "musl"
    && !packages.some((other) => other.os === "linux" && other.cpu === pkg.cpu && other.libc === "glibc");
}

$.logStep(`Building ${npmVersion}...`);

// the repository CI runs in, or else the one with the permanent ID, which
// survives renames
const githubRepository = Deno.env.get("GITHUB_REPOSITORY");
const repositoryUrl = githubRepository != null
  ? `${Deno.env.get("GITHUB_SERVER_URL") ?? "https://github.com"}/${githubRepository}`
  : (await $.request("https://api.github.com/repositories/1092062077").json<{ html_url: string }>()).html_url;
await $`rm -rf ${outputDir}`;
await $`mkdir -p ${dprintDir} ${scopeDir}`;

// setup dprint packages
{
  $.logStep(`Setting up ${mainPackage} ${npmVersion}...`);
  const pkgJson = {
    name: mainPackage,
    version: npmVersion,
    description: "kprint: a pluggable and configurable code formatter written in Rust, forked from dprint.",
    bin: { kprint: "bin.cjs", dprint: "bin.cjs" },
    repository: {
      type: "git",
      url: `git+${repositoryUrl}.git`,
    },
    keywords: ["dprint", "formatter", "format", "code", "cli"],
    author: "Kaj Kowalski",
    license: "MIT",
    bugs: {
      url: `${repositoryUrl}/issues`,
    },
    homepage: "https://dprint.kjanat.dev",
    // for yarn berry (https://github.com/dprint/dprint/issues/686)
    preferUnplugged: true,
    scripts: {
      postinstall: "node ./install.cjs",
    },
    optionalDependencies: Object.fromEntries(
      packages.map((pkg) => [getPackageName(pkg), npmVersion]),
    ),
  };
  for (const entry of ["bin", "install"]) {
    await $`deno bundle --platform=deno --format=cjs --output=${dprintDir.join(`${entry}.cjs`)} ${currentDir.join(`${entry}.ts`)}`;
  }
  dprintDir.join("package.json").writeJsonPrettySync(pkgJson);
  rootDir.join("LICENSE").copyFileSync(dprintDir.join("LICENSE"));
  dprintDir.join("README.md").writeTextSync(mainReadme(repositoryUrl));
  // ensure the test files don't get published
  dprintDir.join(".npmignore").writeTextSync("kprint\nkprint.exe\n");

  // setup each binary package
  const executableHashes: Record<string, string> = {};
  for (const pkg of packages) {
    const pkgName = getPackageNameNoScope(pkg);
    $.logStep(`Setting up ${getPackageName(pkg)}...`);
    const pkgDir = scopeDir.join(platformDir(pkgName));
    const zipPath = pkgDir.join("output.zip");

    await $`mkdir -p ${pkgDir}`;

    // download and extract the zip file
    if (zipsDir == null) {
      const zipUrl = `${repositoryUrl}/releases/download/${version}/${pkg.zipFileName}`;
      await $.request(zipUrl).showProgress().pipeToPath(zipPath);
    } else {
      zipsDir.join(pkg.zipFileName).copyFileSync(zipPath);
    }
    await decompress(zipPath.toString(), pkgDir.toString());
    zipPath.removeSync();
    rootDir.join("LICENSE").copyFileSync(pkgDir.join("LICENSE"));

    // record the executable's hash so the dprint package can verify it when
    // downloading the binary as a fallback (e.g. for `npm install --omit=optional`)
    const executableName = pkg.os === "win32" ? "kprint.exe" : "kprint";
    executableHashes[pkgName] = await sha256Hex(
      pkgDir.join(executableName).toString(),
    );

    // create the package.json and readme
    pkgDir
      .join("README.md")
      .writeTextSync(
        `# ${getPackageName(pkg)}\n\nThe ${
          describePlatform(pkg)
        } binary of [${mainPackage}](https://www.npmjs.com/package/${mainPackage}), which installs it. Install that instead.\n`,
      );
    pkgDir.join("package.json").writeJsonPrettySync({
      name: getPackageName(pkg),
      version: npmVersion,
      description: `The ${describePlatform(pkg)} binary of ${mainPackage}.`,
      repository: {
        type: "git",
        url: `git+${repositoryUrl}.git`,
      },
      // force yarn to unpack
      preferUnplugged: true,
      author: "Kaj Kowalski",
      license: "MIT",
      bugs: {
        url: `${repositoryUrl}/issues`,
      },
      homepage: `https://www.npmjs.com/package/${mainPackage}`,
      os: [pkg.os],
      cpu: [pkg.cpu],
      libc: pkg.libc == null || isPortableLinuxPackage(pkg) ? undefined : [pkg.libc],
    });
  }

  // write the executable hashes into the dprint package so the download
  // fallback can verify the binary it fetches from the registry
  dprintDir.join("hashes.json").writeJsonPrettySync(executableHashes);

  // the `fmt` command, which runs `dprint fmt`
  $.logStep(`Setting up ${fmtPackage} ${npmVersion}...`);
  await $`mkdir -p ${fmtDir}`;
  await $`deno bundle --platform=deno --format=cjs --output=${fmtDir.join("fmt.cjs")} ${currentDir.join("fmt.ts")}`;
  fmtDir.join("package.json").writeJsonPrettySync({
    name: fmtPackage,
    version: npmVersion,
    description: `Runs \`kprint fmt\` (${mainPackage}) as \`fmt\` or \`kfmt\`, with kjanat's configuration by default.`,
    bin: { fmt: "fmt.cjs", kfmt: "fmt.cjs" },
    repository: { type: "git", url: `git+${repositoryUrl}.git` },
    keywords: ["dprint", "formatter", "format", "fmt"],
    author: "Kaj Kowalski",
    license: "MIT",
    bugs: { url: `${repositoryUrl}/issues` },
    homepage: `https://www.npmjs.com/package/${mainPackage}`,
    dependencies: { [mainPackage]: npmVersion },
  });
  rootDir.join("LICENSE").copyFileSync(fmtDir.join("LICENSE"));
  fmtDir.join("README.md").writeTextSync(`# ${fmtPackage}

\`fmt\` (or \`kfmt\`, the same command where \`fmt\` is taken) runs \`kprint fmt\` of
[${mainPackage}](https://www.npmjs.com/package/${mainPackage}) with the arguments it's given.

\`\`\`sh
npx ${fmtPackage}            # format the files of the configuration
npx ${fmtPackage} src/a.ts   # or only these
\`\`\`

Without a configuration, which is no \`--config\` and no \`dprint.json\`,
\`dprint.jsonc\`, \`dprint.toml\` (or a dotted one) in the directory or its
ancestors, it formats with [kjanat's](${fmtDefaultConfig}).

Install ${mainPackage} for the other commands (\`kprint check\`, \`kprint init\`, ...).
`);
}

// verify that the package is created correctly
{
  $.logStep("Verifying packages...");
  const testPlatform = Deno.build.os === "windows"
    ? Deno.build.arch === "x86_64"
      ? platformPackage("win32-x64")
      : platformPackage("win32-arm64")
    : Deno.build.os === "darwin"
    ? Deno.build.arch === "x86_64"
      ? platformPackage("darwin-x64")
      : platformPackage("darwin-arm64")
    : Deno.build.arch === "x86_64"
    ? platformPackage(packages.some((pkg) => pkg.os === "linux" && pkg.cpu === "x64" && pkg.libc === "glibc") ? "linux-x64-glibc" : "linux-x64-musl")
    : platformPackage(packages.some((pkg) => pkg.os === "linux" && pkg.cpu === "arm64" && pkg.libc === "glibc") ? "linux-arm64-glibc" : "linux-arm64-musl");
  $.logLight("Test platform:", testPlatform);
  outputDir.join("package.json").writeJsonPrettySync({
    workspaces: [
      "kprint",
      "fmt",
      // There seems to be a bug with npm workspaces where this doesn't
      // work, so for now make some assumptions and only include the package
      // that works on the CI for the current operating system
      // ...packages.map(getPackageName),
      testPlatform,
    ],
  });

  const dprintExe = Deno.build.os === "windows" ? "kprint.exe" : "kprint";
  await $`npm install`.cwd(dprintDir);

  // ensure the post-install script adds the executable to the dprint package,
  // which is necessary for faster caching and to ensure the vscode extension
  // picks it up
  if (!dprintDir.join(dprintExe).existsSync()) {
    throw new Error("kprint executable did not exist after post install");
  }

  // run once after post install created dprint, once with a simulated readonly file system, once creating the cache and once with
  await $`node bin.cjs -v && rm ${dprintExe} && DPRINT_SIMULATED_READONLY_FILE_SYSTEM=1 node bin.cjs -v && node bin.cjs -v && node bin.cjs -v`.cwd(
    dprintDir,
  );

  if (!dprintDir.join(dprintExe).existsSync()) {
    throw new Error(
      "kprint executable did not exist when lazily initialized",
    );
  }

  // `fmt --help` is `dprint fmt --help`
  await $`node fmt.cjs --help`.cwd(fmtDir);
}

// Beepboop orders a release's publications itself. A nightly publishes the
// tarballs in file name order, platform packages first.
if (args.pack != null) {
  const packDir = $.path(args.pack).resolve();
  packDir.emptyDirSync();
  const directories = [
    ...packages.map((pkg) => ({ dir: scopeDir.join(platformDir(getPackageNameNoScope(pkg))), executable: pkg.os !== "win32" })),
    { dir: dprintDir, executable: false },
    { dir: fmtDir, executable: false },
  ];
  for (const [index, { dir, executable }] of directories.entries()) {
    if (executable) await $`chmod +x ${dir.join("kprint")}`;
    const packed = await $`npm pack ${dir} --ignore-scripts --json --pack-destination ${packDir}`.json<unknown>();
    const filename = packedFilename(packed, dir.toString());
    const name = orderedName(index, filename);
    packDir.join(filename).renameSync(packDir.join(name));
    $.logLight("Packed", name);
  }
}

async function sha256Hex(filePath: string) {
  const data = await Deno.readFile(filePath);
  const digest = await crypto.subtle.digest("SHA-256", data);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** The npm package with the binary (ex. `@kprint/kprint-win32-x64`). */
function getPackageName(pkg: Package) {
  return platformPackage(getPackageNameNoScope(pkg));
}

/** The platform for people (ex. `Linux ppc64le (musl)`). */
function describePlatform(pkg: Package) {
  const os = { win32: "Windows", darwin: "macOS", linux: "Linux", android: "Android", freebsd: "FreeBSD" }[pkg.os];
  // Node reports ppc64le as ppc64
  const cpu = pkg.cpu === "ppc64" ? "ppc64le" : pkg.cpu;
  return pkg.libc == null ? `${os} ${cpu}` : `${os} ${cpu} (${pkg.libc})`;
}

function getPackageNameNoScope(name: Package) {
  const libc = name.libc == null ? "" : `-${name.libc}`;
  return `${name.os}-${name.cpu}${libc}`;
}

function resolveVersion() {
  const firstArg = args._[0];
  if (
    firstArg != null
    && typeof firstArg === "string"
    && firstArg.trim().length > 0
  ) {
    return firstArg;
  }
  const version = rootDir
    .join("crates/kprint/Cargo.toml")
    .readTextSync()
    .match(/^version\s*=\s*"(.*?)"/m)?.[1];
  if (version == null) {
    throw new Error("Could not resolve version.");
  }
  return version;
}
