# Releasing kprint

Stable releases start in Beepboop's `main` stream, through its dashboard or CLI. The stream publishes 16 platform packages, `@kprint/kprint`, `@kjanat/fmt`, and the GitHub release. Cargo publication remains disabled.

1. Choose the release version in Beepboop. `release.yml` applies the version plan to `crates/kprint/Cargo.toml`, regenerates `Cargo.lock`, and submits the version PR.
2. Review that PR and its checks, then merge it manually. Approve promotion in Beepboop for the prepared source commit.
3. Beepboop dispatches `publish.yml` with the execution ID. The workflow verifies the source commit's signature and builds every target in `.github/build-matrix.json` from that exact commit.
4. The binary workflow uploads all 16 kprint archives, their 16 dprint compatibility archives, provenance attestations, and `SHASUMS256.txt` to Beepboop's draft release. Missing targets stop publication.
5. npm packages are built from those artifacts and packed into 18 tarballs, which Verdaccio installs on Linux, macOS, and Windows. Beepboop's `actions/publish` uploads those tarballs with npm trusted publishing from `publish.yml`, platform packages before `@kprint/kprint` and `@kjanat/fmt`. Beepboop verifies each package's registry integrity as npm lists it.
6. Once the binaries are uploaded, the workflow checks the complete uploaded asset set, publishes the signed version tag through Beepboop, and finalizes the GitHub release. npm publication runs beside it. Beepboop verifies the GitHub publication as the nineteenth required destination.

The `release.published` event runs Docker, Homebrew, Scoop, and AUR distribution. It does not start a second npm publication. Scheduled and manually requested nightlies retain the `nightly` npm tag and Docker path; `preview.yml` keeps its existing preview flow.

The stable npm version is exactly the version selected in Beepboop. Use a plain version such as `0.60.8` for `latest`; prerelease versions use `next`. The Cargo manifest supplies the version even though crates.io publication is disabled.

## Retrying a failed release

Beepboop re-runs the failed jobs of a run whose npm packages its ledger can retry, up to the fourth run attempt. Use Beepboop's retry operation for anything else. Failed jobs resume against the same source commit. An already published version is accepted only when its registry integrity matches the tarball from the build; a mismatch stops the release. GitHub stays a draft until asset verification succeeds. Finalization can be retried if GitHub accepted publication before the workflow received its result.

The workflow requires all declared targets. Removing a supported target requires updating the build matrix, npm target descriptor, and Beepboop package/publication lists together.

## Local validation

```sh
deno install --frozen --entrypoint deployment/npm/build.ts deployment/npm/release_test.ts deployment/npm/pack_test.ts deployment/npm/workflow_test.ts
deno task --cwd deployment/npm check
deno task --cwd deployment/npm test
actionlint .github/workflows/release.yml .github/workflows/binaries.yml .github/workflows/publish.yml .github/workflows/ci.yml
```

These checks validate the tarball order, the nightly upload order, complete binary assets, and the stable/nightly/distribution planning routes without publishing anything. The release workflow additionally runs the three-platform npm installation tests before npm publication.
