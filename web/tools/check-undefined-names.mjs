// Fails on any name the web code uses but never defines. Vite builds such
// code without complaint and the browser throws only when the line runs, so
// this is the one slice of svelte-check that is a gate while the full type
// check still has a baseline.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");

// Names that are defined, just not where svelte-check looks.
const ALLOWED = [
  // Replaced at build time by Vite `define`.
  { file: "src/agent/frame.js", name: "__KATEX__" },
  { file: "src/site/SiteBar.svelte", name: "__APP_ORIGIN__" },
  // Node's Buffer, in code paths that also run under Node.
  { file: null, name: "Buffer" },
];

const run = spawnSync(join(web, "node_modules/.bin/svelte-check"),
  ["--tsconfig", "./jsconfig.json", "--output", "machine"], { cwd: web, encoding: "utf8" });
if (run.error || run.stdout == null) {
  console.error(`check-undefined-names: could not run svelte-check: ${run.error?.message || run.stderr}`);
  process.exit(2);
}

// Machine output: <timestamp> ERROR "<file>" <line>:<column> "<message>"
const LINE = /^\d+ ERROR "([^"]+)" (\d+:\d+) "Cannot find name '([^']+)'/;
const offending = run.stdout.split("\n").flatMap((line) => {
  const found = line.match(LINE);
  if (!found) return [];
  const [, file, at, name] = found;
  const allowed = ALLOWED.some((entry) => entry.name === name && (entry.file === null || entry.file === file));
  return allowed ? [] : [`${file}:${at} ${name}`];
});

if (offending.length) {
  console.error("undefined names:");
  for (const line of offending) console.error(`  ${line}`);
  process.exit(1);
}
console.log("undefined names: none");
