// These tests drive the built shell, and a stale build tests code that is no longer there.
// Fail loudly when web/dist is older than its inputs instead of testing old code.
import { existsSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const WEB = fileURLToPath(new URL("../../", import.meta.url));

const OUTPUTS = ["dist/index.html", "dist/viewer.html", "dist/frame.js"];
const INPUT_DIRS = ["src", "pages"];
const INPUT_FILES = ["vite.config.js", "vite.frame.config.js"];
// The documentation site is built on its own and is not part of web/dist.
const NOT_INPUTS = [join(WEB, "src", "site")];

export function assertFreshDist(label) {
  const built = OUTPUTS.map((path) => join(WEB, path))
    .filter((path) => existsSync(path))
    .map((path) => statSync(path).mtimeMs);
  if (built.length === 0) return;
  const buildTime = Math.min(...built);

  let newest = null;
  let newestTime = -Infinity;
  const consider = (file) => {
    const time = statSync(file).mtimeMs;
    if (time > newestTime) {
      newest = file;
      newestTime = time;
    }
  };

  for (const dir of INPUT_DIRS) {
    const root = join(WEB, dir);
    if (!existsSync(root)) continue;
    for (const entry of readdirSync(root, { recursive: true, withFileTypes: true })) {
      const file = join(entry.parentPath, entry.name);
      if (entry.isDirectory() || NOT_INPUTS.some((skip) => file.startsWith(skip))) continue;
      consider(file);
    }
  }
  for (const file of INPUT_FILES) consider(join(WEB, file));

  if (newestTime > buildTime) {
    throw new Error(`${label}: web/dist is older than ${relative(WEB, newest)}; run \`bun run build\` in web/ first`);
  }
}
