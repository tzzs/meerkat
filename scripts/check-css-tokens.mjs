// Fails when a stylesheet reads a custom property nobody defines.
//
//   node scripts/check-css-tokens.mjs
//
// A `var(--missing)` makes the whole declaration invalid at computed value time, so the
// property silently falls back to its initial value: a border disappears, a scrollbar thumb
// goes transparent. The palette rename in a9b57af left 21 such reads behind, which is why
// every hairline rule in the window had stopped drawing.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = process.cwd();
const STYLE_DIRS = ["src/styles"];
const SOURCE_DIR = "src";

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) walk(path, out);
    else out.push(path);
  }
  return out;
}

const styleFiles = STYLE_DIRS.flatMap((dir) => walk(join(ROOT, dir)).filter((f) => f.endsWith(".css")));
const sourceFiles = walk(join(ROOT, SOURCE_DIR)).filter((f) => f.endsWith(".tsx"));

const defined = new Set();
const definedBy = new Map();
const read = [];
const readBy = [];

for (const file of [...styleFiles, ...sourceFiles]) {
  const text = readFileSync(file, "utf8");
  const where = relative(ROOT, file).replaceAll("\\", "/");
  // A definition is `--name:` in a stylesheet, or `"--name"` in a style object the UI sets.
  for (const match of text.matchAll(/(^|[\s{;,(])--([a-z0-9-]+)\s*:/gi)) {
    const name = `--${match[2].toLowerCase()}`;
    if (!defined.has(name)) {
      defined.add(name);
      definedBy.set(name, where);
    }
  }
  for (const match of text.matchAll(/"(--[a-z0-9-]+)"/gi)) {
    defined.add(match[1].toLowerCase());
  }
  if (file.endsWith(".css")) {
    for (const match of text.matchAll(/var\(\s*(--[a-z0-9-]+)\s*(,)?/gi)) {
      const name = match[1].toLowerCase();
      // A fallback makes the read safe by design.
      if (match[2] === ",") continue;
      read.push(name);
      readBy.push(`${where}: ${name}`);
    }
  }
}

const dangling = [...new Set(read.filter((name) => !defined.has(name)))].sort();

if (dangling.length > 0) {
  console.log(`undefined custom properties read by stylesheets (${dangling.length}):`);
  for (const name of dangling) {
    const uses = readBy.filter((line) => line.endsWith(`: ${name}`));
    console.log(`  ${name}  read ${uses.length}x, e.g. ${uses[0]}`);
  }
  process.exit(1);
}

const total = new Set(read).size;
console.log(`every custom property a stylesheet reads is defined (${total} distinct names)`);
