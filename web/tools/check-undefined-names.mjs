import { spawnSync } from "child_process";
import { fileURLToPath } from "url";
import { dirname, resolve } from "path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const webDir = resolve(__dirname, "..");

// Build-time constants defined by Vite `define` plugin
// and Node's Buffer used in code paths that run under Node
const allowlist = new Map([
  ["src/agent/frame.js", new Set(["__KATEX__"])],
  ["src/site/SiteBar.svelte", new Set(["__APP_ORIGIN__"])],
]);

// Any file with Buffer is allowed (used in Node code paths)
const allowBufferInAllFiles = true;

const result = spawnSync(
  "svelte-check",
  ["--tsconfig", "./jsconfig.json", "--output", "machine"],
  {
    cwd: webDir,
    encoding: "utf-8",
  }
);

const lines = result.stdout.split("\n").filter((line) => line.trim());
const errors = lines.filter(
  (line) =>
    line.includes("ERROR") && line.includes("Cannot find name")
);

if (errors.length === 0) {
  console.log("undefined names: none");
  process.exit(0);
}

const offending = [];

for (const line of errors) {
  // Parse the machine format: file:line:col - ERROR Cannot find name 'xyz'
  // Example: src/agent/frame.js:123:45 - ERROR Cannot find name '__KATEX__'
  const match = line.match(
    /^([^\s]+)[:\s]+ERROR\s+Cannot find name '([^']+)'/
  );

  if (!match) continue;

  const [, filePath, name] = match;
  const isAllowlisted =
    (allowlist.has(filePath) && allowlist.get(filePath).has(name)) ||
    (allowBufferInAllFiles && name === "Buffer");

  if (!isAllowlisted) {
    offending.push(line);
  }
}

if (offending.length > 0) {
  console.error("undefined names found:");
  for (const line of offending) {
    console.error(line);
  }
  process.exit(1);
}

console.log("undefined names: none");
