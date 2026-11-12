import targets from "./targets.json" with { type: "json" };

// The npm packages the release is published as, which build.ts creates and
// install_api.ts looks the binary up in.

/** The scope of the packages. */
export const scope = targets.scope;

/** The package users install, with the `dprint` and `kprint` commands. */
export const mainPackage = targets.facades[0].name;

export const fmtPackage = targets.facades[0].shim;

/** The prefix of the platform packages, also of their tarball file names. */
const platformPrefix = targets.facades[0].pkg;

/**
 * The package with the binary for a platform (ex. `@kprint/kprint-win32-x64`
 * for `win32-x64`), which is also its directory in the scope's.
 */
export function platformPackage(platform: string): string {
  return `${scope}/${platformDir(platform)}`;
}

/** The directory, and tarball file name, of a platform's package. */
export function platformDir(platform: string): string {
  return `${platformPrefix}-${platform}`;
}
