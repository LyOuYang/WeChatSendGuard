# Group Bookmarks AI Assets

AI agents write import batches only. The IDE plugin imports those batches, resolves real source locations, and writes `.group-bookmarks/staged-bookmarks.json` itself.

## Entry Points

Use the skills under `.group-bookmarks/ai/skill/`:

- `stage-bookmarks-full`: scan the full codebase.
- `stage-bookmarks-changes`: scan current uncommitted git changes.
- `stage-bookmarks-workflow`: scan a user-specified workflow or logic area.

## Hard Rules

- Do not write `.group-bookmarks/bookmarks.json`.
- Do not write `.group-bookmarks/staged-bookmarks.json`.
- Treat existing bookmarks as the current project navigation map, not as a separate project-map document.
- Output a concise plan and wait for user confirmation before writing import batches.
- If bookmark language is ambiguous, ask exactly `请选择书签语言：中文 / English`; group names and bookmark titles must use the same chosen language.
- Write import batches to `.group-bookmarks/drop-zone/inbox/*.tmp`.
- Import batch `.tmp` and `.json` files must be UTF-8.
- Include `expectedLineText` only when the source line was read with the correct project encoding; omit it if encoding is uncertain.
- Validate and promote each batch with `.group-bookmarks/ai/scripts/validate-import-batch.js`.
- If the validator is missing, stop and tell the user to run the Group Bookmarks AI assets repair command.

## Validation

```bash
node .group-bookmarks/ai/scripts/validate-import-batch.js --promote .group-bookmarks/drop-zone/inbox/<batch-file>.tmp
```

On success, the validator renames `.tmp` to `.json`. On failure, it exits non-zero, prints `ERROR:` and `Next:` guidance, and removes the invalid `.tmp` in `--promote` mode.

## Import Batch Shape

```json
{
  "targetGroupName": "Architecture",
  "items": [
    {
      "title": "Repository boundary",
      "filePath": "src/data/repositoryStore.ts",
      "line": 20,
      "expectedLineText": "static readonly sidecarDirectoryName = '.group-bookmarks';"
    }
  ]
}
```

Allowed fields are `targetGroupName`, `items`, `title`, `filePath`, `line`, `expectedLineText`, `placement`, `afterTitle`, and `beforeTitle`.
