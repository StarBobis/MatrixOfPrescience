export interface PatchStats {
  files: number;
  added: number;
  deleted: number;
}

/**
 * Parses a unified diff and returns how many files it touches and the
 * added/removed line counts, so the UI can show "N files changed, +X, -Y"
 * without re-walking the patch in every component.
 */
export function parsePatchStats(patchText: string): PatchStats {
  let files = 0;
  let added = 0;
  let deleted = 0;

  for (const line of patchText.split("\n")) {
    if (line.startsWith("diff --git ")) {
      files += 1;
      continue;
    }

    // File headers are not content lines.
    if (line.startsWith("+++") || line.startsWith("---")) {
      continue;
    }

    if (line.startsWith("+")) {
      added += 1;
      continue;
    }

    if (line.startsWith("-")) {
      deleted += 1;
    }
  }

  // Patches without `diff --git` headers (plain `---`/`+++` form) still
  // deserve a file count.
  if (files === 0) {
    files = patchText.split("\n").filter((line) => line.startsWith("+++ ")).length;
  }

  return { files, added, deleted };
}
