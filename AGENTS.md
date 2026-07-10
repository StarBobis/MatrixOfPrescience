# Agent Instructions

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call -- the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely -- indexing is the user's decision.
<!-- CODEGRAPH_END -->

## Editing With `apply_patch`

Before using `apply_patch` to modify an existing file, read the exact location that will be changed and verify the current on-disk content. Use the freshest available source, preferably CodeGraph for indexed code or a direct file read for docs/config.

Only write an `apply_patch` hunk after you know the exact surrounding lines you are replacing or inserting near. Anchor patches on exact old content and stable nearby context, not on remembered or hand-guessed line numbers. Do not rely on stale context, previous tool output, or assumptions about formatting. If the file changed after you last read it, read the target location again before patching.

Avoid hand-writing fragile unified diff hunk ranges such as `@@ -5144,7 +5144,7 @@` unless they were generated from freshly read content and you have checked the hunk body counts. For non-trivial patches, first run a check-only validation when the tool supports it, then apply the same patch after it passes.

For new files, first confirm the target path does not already exist. For generated or bulk formatter changes, confirm the formatter output or generated files before making further manual patches.
