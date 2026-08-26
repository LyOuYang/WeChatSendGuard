---
name: stage-bookmarks-full
description: "Scan the full codebase, propose a concise bookmark grouping plan for confirmation, then generate validated Group Bookmarks bookmark candidate import JSON files in .group-bookmarks/drop-zone/inbox/. The only final artifacts are promoted .json files; never create Markdown docs, workflow directories, project maps, .group-bookmarks/staged-bookmarks.json, or .group-bookmarks/bookmarks.json. Use only when the user explicitly asks to initialize, rebuild, or broadly refresh project bookmarks."
---

# Stage Bookmarks — Full Codebase Import

Use this skill for a full codebase scan.

## One-Screen Contract

- Output only bookmark candidate import lists: strict JSON files saved first as `.group-bookmarks/drop-zone/inbox/<group-slug>.tmp`, then promoted to `.json`.
- Create one final `.json` file per confirmed group.
- Before confirmation, respond only in chat. Do not write files.
- Group by maintenance workflow, not by package names, file tree, or documentation sections.
- Never create architecture folders, Markdown notes, analysis reports, project maps, `.group-bookmarks/staged-bookmarks.json`, or `.group-bookmarks/bookmarks.json`.
- Use only the project-local validator: `.group-bookmarks/ai/scripts/validate-import-batch.js`.

## Hard Rules

1. Wait for user confirmation before writing any `.tmp` or `.json` file.
2. Create only `.group-bookmarks/drop-zone/inbox/*.tmp` as validator input and only `.group-bookmarks/drop-zone/inbox/*.json` as final output.
3. Create one final `.json` file per confirmed group.
4. Do not create directories, Markdown files, reports, project maps, `.group-bookmarks/staged-bookmarks.json`, or `.group-bookmarks/bookmarks.json`.
5. Group by maintenance workflow, not by package names or file tree.
6. Use only JSON fields: `targetGroupName`, `items`, `title`, `filePath`, `line`, `expectedLineText`, `placement`, `afterTitle`, `beforeTitle`.
7. Do not generate IDs, colors, order keys, anchors, status, source, confidence, reasons, links, or related groups.
8. `.tmp` file content must be strict JSON only: no Markdown fences, comments, prose, or trailing commas.
9. `.tmp` and `.json` import batch files must be UTF-8.

## Language

- Infer bookmark language from the user's latest explicit request.
- If the user explicitly asks for Chinese or English bookmarks, obey that choice.
- Otherwise, use the dominant natural language of the user's request.
- If the bookmark language is ambiguous, stop before the Bookmark Group Plan. Ask the user exactly: `请选择书签语言：中文 / English`.
- Once the language is selected or inferred, use that single language for both `targetGroupName` and every item `title`.
- Group names and bookmark titles must use the same chosen language.
- Do not mix Chinese group names with English bookmark titles, or English group names with Chinese bookmark titles.
- Keep code identifiers, file paths, class names, method names, API names, and `expectedLineText` unchanged.
- Include `Bookmark language: 中文` or `Bookmark language: English` in the Bookmark Group Plan.

## Phase 1: Lightweight Project Scan

Read existing bookmarks as the current project navigation map.

Inspect only enough context to create a grouping plan:

- `.group-bookmarks/bookmarks.json`
- README, docs index, or project overview if present
- build/config files and module structure
- manifests, routes, plugin descriptors, package entrypoints, or equivalent runtime entrypoints
- main source tree file names
- test file names
- selected files that look like entrypoints, core services, repositories, UI/API surfaces, workers, schedulers, integrations, diagnostics, or high-value tests

Do not read every file upfront. Code is truth; existing bookmarks are the current navigation result.

## Phase 2: Bookmark Group Plan

Before writing any file, output a concise Bookmark Group Plan in chat only and stop for confirmation.

The plan must be at most 80 lines.

Format:

```text
Bookmark Group Plan
1. Bookmark language: <中文|English>
2. Project shape: <one sentence>
3. Proposed groups:
   - <Group name>: <why this group exists>; expected <N> bookmarks; sample targets: <file/class names>
4. Estimated total bookmarks: <N>
5. Skipped or intentionally low-value areas: <short list>
6. Confirm this grouping plan? I will generate one final bookmark candidate import JSON file per confirmed group, with no Markdown files, staged-bookmarks.json, or bookmarks.json.
```

Group by maintenance workflow, not by package names or file tree.

Candidate dimensions:

- Entry points and lifecycle
- Core domain workflow
- Data ingestion and parsing
- Business rules and processing
- Persistence and state
- UI/API/CLI surfaces
- Background work and scheduling
- External integrations and protocols
- Configuration and environment
- Error handling, diagnostics, and reliability
- Security and permissions
- Tests and executable specifications

Only create groups that fit the project. Usually create 4-8 groups.

Quantity guidance:

- Tiny project: 8-12 bookmarks
- Small project: 12-20 bookmarks
- Medium project: 20-35 bookmarks
- Large project: 35-60 bookmarks
- Monorepo or multi-module project: 8-15 per major module, capped unless the user asks for exhaustive coverage

Per group, usually target 3-8 bookmarks. Merge groups with fewer than 3 useful candidates; split groups with more than 8 strong candidates.

## Phase 3: Generate Confirmed JSON Files

After the user confirms the Bookmark Group Plan:

1. Create one `.group-bookmarks/drop-zone/inbox/<group-slug>.tmp` strict JSON file per confirmed group.
2. Validate and promote each batch with the project-local validator:

```bash
node .group-bookmarks/ai/scripts/validate-import-batch.js --promote .group-bookmarks/drop-zone/inbox/<group-slug>.tmp
```

If `.group-bookmarks/ai/scripts/validate-import-batch.js` is missing, stop and tell the user to run the Group Bookmarks AI assets repair command. Do not infer global skill paths.

3. If validation exits non-zero, read the `ERROR:` lines, fix the batch, recreate the `.tmp`, and rerun validation. In `--promote` mode the validator removes the invalid `.tmp`; do not leave failed `.tmp` files in the inbox.
4. Report generated `.json` files and candidate counts.
5. The final filesystem output must be only the promoted `.json` import file(s) under `.group-bookmarks/drop-zone/inbox/`.

This skill is a project-local instruction file. The shared validator lives at `.group-bookmarks/ai/scripts/validate-import-batch.js`.

## Candidate Quality

Each bookmark must answer at least one maintenance question:

- Where does this workflow start?
- Where does input enter the system?
- Where is data parsed, validated, transformed, or persisted?
- Where are core decisions made?
- Where does external integration happen?
- Where does UI/API/CLI output get assembled?
- Where is background work scheduled or executed?
- Where are failures handled or diagnosed?
- Which tests explain the expected behavior?

Avoid passive data-only classes, trivial wrappers, constants-only files, generated code, broad package markers, and duplicate call-through points when a deeper implementation boundary is more useful.

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
