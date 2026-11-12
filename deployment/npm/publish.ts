export interface PackedPackage {
  name: string;
  version: string;
  integrity: string;
  filename: string;
}
export interface RegistryPackage {
  name: string;
  version: string;
  dist: { integrity: string };
}
export interface Publication {
  id: string;
  manager: "npm";
  name: string;
  version: string;
  digest: string;
  required: boolean;
  outcome: "succeeded";
}

export function packedPackage(result: unknown, name: string, version: string): PackedPackage {
  const entries = result && typeof result === "object" ? Object.values(result) : [];
  const pkg = entries[0] as Partial<PackedPackage> | undefined;
  if (
    entries.length !== 1 || !pkg || pkg.name !== name || pkg.version !== version || typeof pkg.integrity !== "string"
    || !pkg.integrity.startsWith("sha512-") || typeof pkg.filename !== "string" || !/^[^/\\]+\.tgz$/.test(pkg.filename)
  ) throw new Error(`Unexpected npm pack result for ${name}@${version}`);
  return pkg as PackedPackage;
}

export function verifyPackage(expected: PackedPackage, actual: RegistryPackage) {
  if (actual.name !== expected.name || actual.version !== expected.version || actual.dist?.integrity !== expected.integrity) {
    throw new Error(`Registry contents differ from the built package: ${expected.name}@${expected.version}`);
  }
}

export async function publishVerified(
  pkg: PackedPackage,
  read: () => Promise<RegistryPackage | undefined>,
  publish: () => Promise<void>,
  wait: () => Promise<void> = () => new Promise((resolve) => setTimeout(resolve, 2000)),
): Promise<Publication> {
  let actual = await read();
  if (actual) verifyPackage(pkg, actual);
  else {
    await publish();
    for (let attempt = 0; attempt < 60; attempt++) {
      actual = await read();
      if (actual) break;
      await wait();
    }
    if (!actual) throw new Error(`Publication not visible on npm: ${pkg.name}@${pkg.version}`);
    verifyPackage(pkg, actual);
  }
  return {
    id: `pkg:npm/${pkg.name.split("/").map(encodeURIComponent).join("/")}`,
    manager: "npm",
    name: pkg.name,
    version: pkg.version,
    digest: pkg.integrity,
    required: true,
    outcome: "succeeded",
  };
}

export async function readPackage(name: string, version: string): Promise<RegistryPackage | undefined> {
  const response = await fetch(`https://registry.npmjs.org/${encodeURIComponent(name)}/${encodeURIComponent(version)}`);
  if (response.status === 404) {
    await response.body?.cancel();
    return undefined;
  }
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error(`npm returned HTTP ${response.status} for ${name}@${version}`);
  }
  return await response.json();
}
