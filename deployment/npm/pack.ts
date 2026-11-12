export function packedFilename(result: unknown, dir: string): string {
  const entries: unknown[] = result !== null && typeof result === "object" ? Object.values(result) : [];
  const [entry] = entries;
  if (
    entries.length === 1 && entry !== null && typeof entry === "object" && "filename" in entry
    && typeof entry.filename === "string" && /^[^/\\]+\.tgz$/.test(entry.filename)
  ) return entry.filename;
  throw new Error(`Unexpected npm pack result for ${dir}`);
}

export function orderedName(index: number, filename: string): string {
  return `${String(index).padStart(2, "0")}-${filename}`;
}
