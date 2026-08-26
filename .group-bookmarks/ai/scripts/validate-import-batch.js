#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const { TextDecoder } = require("util");

const STRICT_UTF8_DECODER = new TextDecoder("utf-8", { fatal: true });
const REPLACEMENT_CHARACTER = "\uFFFD";

const args = process.argv.slice(2);
const promote = args.includes("--promote");
const unknownOptions = args.filter((arg) => arg.startsWith("--") && arg !== "--promote");
const filePath = args.find((arg) => !arg.startsWith("--"));

if (!filePath) {
  fatal("Usage: node validate-import-batch.js [--promote] <import-batch.tmp|json>");
}

if (unknownOptions.length > 0) {
  fatal(`Unknown option(s): ${unknownOptions.join(", ")}`);
}

let raw;
let rawBytes;
try {
  rawBytes = fs.readFileSync(filePath);
} catch (error) {
  fatal(`Cannot read file: ${error.message}`);
}

try {
  raw = STRICT_UTF8_DECODER.decode(rawBytes);
} catch (error) {
  fatal(`File must be UTF-8: ${error.message}`, { cleanInvalidTemp: promote });
}

let batch;
try {
  batch = JSON.parse(raw);
} catch (error) {
  fatal(`JSON syntax error: ${error.message}`, { cleanInvalidTemp: promote });
}

const errors = [];
const warnings = [];
const ROOT_FIELDS = new Set(["targetGroupName", "items"]);
const ITEM_FIELDS = new Set(["title", "filePath", "line", "expectedLineText", "placement"]);
const PLACEMENT_FIELDS = new Set(["afterTitle", "beforeTitle"]);

if (!isObject(batch) || Array.isArray(batch)) {
  error("Root must be an object.");
} else {
  warnUnsupported(batch, ROOT_FIELDS, "root");
  if (!nonEmptyString(batch.targetGroupName)) {
    error("targetGroupName must be a non-empty string.");
  } else {
    rejectReplacementCharacter(batch.targetGroupName, "targetGroupName");
  }
  if (!Array.isArray(batch.items) || batch.items.length === 0) {
    error("items must be a non-empty array.");
  } else {
    batch.items.forEach((item, index) => validateItem(item, index));
  }
}

console.log(formatReport());
if (errors.length > 0) {
  if (promote) {
    removeInvalidTemp(filePath);
  }
  process.exit(1);
}

if (promote) {
  promoteTempFile(filePath);
}

process.exit(0);

function validateItem(item, index) {
  const label = `items[${index}]`;
  if (!isObject(item) || Array.isArray(item)) {
    error(`${label} must be an object.`);
    return;
  }
  warnUnsupported(item, ITEM_FIELDS, label);
  if (!nonEmptyString(item.title)) {
    error(`${label}.title must be a non-empty string.`);
  } else {
    rejectReplacementCharacter(item.title, `${label}.title`);
  }
  if (!nonEmptyString(item.filePath)) {
    error(`${label}.filePath must be a non-empty string.`);
  } else {
    validateFilePath(item.filePath, label);
    rejectReplacementCharacter(item.filePath, `${label}.filePath`);
  }
  if (!Number.isInteger(item.line) || item.line < 1) {
    error(`${label}.line must be an integer >= 1.`);
  }
  if (item.expectedLineText !== undefined && typeof item.expectedLineText !== "string") {
    error(`${label}.expectedLineText must be a string when present.`);
  } else if (typeof item.expectedLineText === "string") {
    rejectReplacementCharacter(item.expectedLineText, `${label}.expectedLineText`);
  }
  if (item.placement !== undefined) {
    if (!isObject(item.placement) || Array.isArray(item.placement)) {
      error(`${label}.placement must be an object when present.`);
    } else {
      warnUnsupported(item.placement, PLACEMENT_FIELDS, `${label}.placement`);
      if (typeof item.placement.afterTitle === "string") {
        rejectReplacementCharacter(item.placement.afterTitle, `${label}.placement.afterTitle`);
      }
      if (typeof item.placement.beforeTitle === "string") {
        rejectReplacementCharacter(item.placement.beforeTitle, `${label}.placement.beforeTitle`);
        error(`${label}.placement.beforeTitle is not supported; use placement.afterTitle.`);
      }
    }
  }
}

function validateFilePath(value, label) {
  if (value.includes("\\")) {
    error(`${label}.filePath must use forward slashes, not backslashes.`);
  }
  if (path.isAbsolute(value)) {
    error(`${label}.filePath must be project-relative, not absolute.`);
  }
  if (value.split("/").includes("..")) {
    error(`${label}.filePath must not contain '..'.`);
  }
}

function warnUnsupported(object, allowed, label) {
  Object.keys(object).forEach((key) => {
    if (!allowed.has(key)) {
      warning(`${label}.${key} is unsupported and will be ignored by the IDE import workflow.`);
    }
  });
}

function rejectReplacementCharacter(value, label) {
  if (value.includes(REPLACEMENT_CHARACTER)) {
    error(`${label} contains Unicode replacement character; regenerate the batch from correctly decoded source text.`);
  }
}

function formatReport() {
  const lines = ["Group Bookmarks import batch validation"];
  if (errors.length === 0 && warnings.length === 0) {
    lines.push("All checks passed.");
    return lines.join("\n");
  }
  errors.forEach((message) => lines.push(`ERROR: ${message}`));
  warnings.forEach((message) => lines.push(`WARN: ${message}`));
  if (errors.length > 0) {
    lines.push("Next: fix the batch, recreate the .tmp file, rerun validation, and only promote to .json after all errors are gone.");
    if (promote) {
      lines.push("In --promote mode, the invalid .tmp file is removed before exit.");
    }
  }
  return lines.join("\n");
}

function error(message) {
  errors.push(message);
}

function warning(message) {
  warnings.push(message);
}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function isObject(value) {
  return value !== null && typeof value === "object";
}

function promoteTempFile(inputPath) {
  if (path.extname(inputPath).toLowerCase() !== ".tmp") {
    fatal("--promote requires a .tmp input file.");
  }
  const parsed = path.parse(inputPath);
  const outputPath = path.join(parsed.dir, `${parsed.name}.json`);
  if (fs.existsSync(outputPath)) {
    fatal(`Refusing to overwrite existing output file: ${outputPath}`);
  }
  try {
    fs.renameSync(inputPath, outputPath);
    console.log(`Promoted: ${inputPath} -> ${outputPath}`);
  } catch (error) {
    fatal(`Cannot promote file: ${error.message}`);
  }
}

function removeInvalidTemp(inputPath) {
  if (!inputPath || path.extname(inputPath).toLowerCase() !== ".tmp") {
    return;
  }
  try {
    if (fs.existsSync(inputPath)) {
      fs.unlinkSync(inputPath);
      console.error(`Removed invalid temp file: ${inputPath}`);
    }
  } catch (error) {
    console.error(`WARN: Could not remove invalid temp file: ${error.message}`);
  }
}

function fatal(message, options = {}) {
  console.error(`ERROR: ${message}`);
  if (options.cleanInvalidTemp) {
    removeInvalidTemp(filePath);
  }
  process.exit(1);
}
