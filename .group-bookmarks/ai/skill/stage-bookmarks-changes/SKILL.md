---
name: stage-bookmarks-changes
description: "Inspect current uncommitted git changes, propose a concise changed-workflow bookmark plan for confirmation, then generate validated Group Bookmarks bookmark candidate import JSON files in .group-bookmarks/drop-zone/inbox/. The only final artifacts are promoted .json files; never create Markdown docs, workflow directories, project maps, .group-bookmarks/staged-bookmarks.json, or .group-bookmarks/bookmarks.json. Use only when the user explicitly asks to mark recent changes."
---

# Stage Bookmarks — Changes Import

Use this skill for uncommitted git changes.

## One-Screen Contract

- Output only bookmark candidate import lists: strict JSON files saved first as `.group-bookmarks/drop-zone/inbox/<group-slug>-changes.tmp`, then promoted to `.json`.
- Create one final `.json` file per confirmed changed-workflow group.
- Before confirmation, respond only in chat. Do not write files.
- Bookmark only changed locations or nearby definitions needed to understand changed symbols.
- Never create workflow folders, Markdown notes, analysis reports, project maps, `.group-bookmarks/staged-bookmarks.json`, or `.group-bookmarks/bookmarks.json`.
- Use only the project-local validator: `.group-bookmarks/ai/scripts/validate-import-batch.js`.

## Hard Rules

1. Wait for user confirmation before writing any `.tmp` or `.json` file.
2. Create only `.group-bookmarks/drop-zone/inbox/*.tmp` as validator input and only `.group-bookmarks/drop-zone/inbox/*.json` as final output.
3. Create one final `.json` file per confirmed changed-workflow group.
4. Do not create directories, Markdown files, reports, project maps, `.group-bookmarks/staged-bookmarks.json`, or `.group-bookmarks/bookmarks.json`.
5. Only propose bookmarks for touched locations or immediately necessary nearby definitions.
6. Use only JSON fields: `targetGroupName`, `items`, `title`, `filePath`, `line`, `expectedLineText`, `placement`, `afterTitle`, `beforeTitle`.
7. Do not generate IDs, colors, order keys, anchors, status, source, confidence, reasons, links, or related groups.
8. `.tmp` file content must be strict JSON only: no Markdown fences, comments, prose, or trailing commas.
9. `.tmp` and `.json` import batch files must be UTF-8.

## Language

- Infer bookmark language from the user's latest explicit request.
- If the user explicitly asks for Chinese or English bookmarks, obey that choice.
- Otherwise, use the dominant natural language of the user's request.
- If the bookmark language is ambiguous, stop before the Changes Bookmark Plan. Ask the user exactly: `请选择书签语言：中文 / English`.
- Once the language is selected or inferred, use that single language for both `targetGroupName` and every item `title`.
- Group names and bookmark titles must use the same chosen language.
- Do not mix Chinese group names with English bookmark titles, or English group names with Chinese bookmark titles.
- Keep code identifiers, file paths, class names, method names, API names, and `expectedLineText` unchanged.
- Include `Bookmark language: 中文` or `Bookmark language: English` in the Changes Bookmark Plan.

## Phase 1: Diff Scan

Read existing bookmarks as the current project navigation map.

Inspect:

- `.group-bookmarks/bookmarks.json`
- `git status --short`
- `git diff --stat`
- relevant `git diff <file>` and `git diff --cached <file>` hunks
- nearby definitions only when needed to understand a changed symbol

Do not scan unrelated project files. If there are no uncommitted git changes, stop and tell the user there is nothing to bookmark.

## Phase 2: Changes Bookmark Plan

Before writing any file, output a concise Changes Bookmark Plan in chat only and stop for confirmation.

The plan must be at most 50 lines.

Format:

```text
Changes Bookmark Plan
1. Bookmark language: <中文|English>
2. Change shape: <one sentence>
3. Proposed groups:
   - <Group name>: <changed workflow>; expected <N> bookmarks; touched files: <short list>
4. Skipped changes:
   - <file/pattern>: <reason>
5. Estimated total bookmarks: <N>
6. Confirm this changes bookmark plan? I will generate one final bookmark candidate import JSON file per confirmed changed-workflow group, with no Markdown files, staged-bookmarks.json, or bookmarks.json.
```

Group by changed workflow, not by file path.

Typical groups:

- Feature entrypoints
- Changed domain logic
- Persistence/schema changes
- UI/API/CLI changes
- Background jobs/schedulers
- Integration/protocol changes
- Tests and executable specifications

If the diff is small, one group is fine. If unrelated areas changed, create multiple groups.

Quantity guidance:

- Tiny diff: 1-3 bookmarks
- Small diff: 3-6 bookmarks
- Medium diff: 6-12 bookmarks
- Large feature diff: 12-20 bookmarks

## Phase 3: Generate Confirmed JSON Files

After the user confirms the Changes Bookmark Plan:

1. Create one `.group-bookmarks/drop-zone/inbox/<group-slug>-changes.tmp` strict JSON file per confirmed group.
2. Validate and promote each batch with the project-local validator:

```bash
node .group-bookmarks/ai/scripts/validate-import-batch.js --promote .group-bookmarks/drop-zone/inbox/<group-slug>-changes.tmp
```

If `.group-bookmarks/ai/scripts/validate-import-batch.js` is missing, stop and tell the user to run the Group Bookmarks AI assets repair command. Do not infer global skill paths.

3. If validation exits non-zero, read the `ERROR:` lines, fix the batch, recreate the `.tmp`, and rerun validation. In `--promote` mode the validator removes the invalid `.tmp`; do not leave failed `.tmp` files in the inbox.
4. Report generated `.json` files, candidate counts, and skipped changed files.
5. The final filesystem output must be only the promoted `.json` import file(s) under `.group-bookmarks/drop-zone/inbox/`.

This skill is a project-local instruction file. The shared validator lives at `.group-bookmarks/ai/scripts/validate-import-batch.js`.

## Candidate Quality

Bookmark changed locations that answer:

- Where does the new or changed behavior start?
- Where is the important decision made?
- Where is state changed or persisted?
- Where does UI/API/CLI output change?
- Which test explains the behavior?

Skip formatting-only files, generated files, renames without behavior change, comments-only edits, trivial strings, and resource-only changes unless they define important UI behavior.

## Standard Import JSON Shape

The generated file must be one JSON object with this standard Group Bookmarks import shape.

Required root fields:

- `targetGroupName`: target bookmark group name for this import file.
- `items`: non-empty bookmark candidate array.

Required item fields:

- `title`: bookmark title shown in the target group.
- `filePath`: project-relative path using `/`.
- `line`: 1-based line number.

Optional item fields:

- `expectedLineText`: expected source text at `line`; include it only when the source line was read with the correct project encoding; omit it if encoding is uncertain.
- `placement.afterTitle` / `placement.beforeTitle`: ordering hint inside the target group; use only when the confirmed plan needs relative placement.

```json
{
  "targetGroupName": "Example Workflow",
  "items": [
    {
      "title": "Example entrypoint",
      "filePath": "src/example.ts",
      "line": 10,
      "expectedLineText": "export function example() {"
    }
  ]
}
```
