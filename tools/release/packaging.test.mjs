// Runs packaging.test.sh: offline checks of the Homebrew and Scoop updaters and
// the deploy kit packager, which otherwise first run after a release.
import { execFileSync } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "packaging.test.sh");

test("updaters and the kit packager verify, generate and push", () => {
  execFileSync("bash", [script], { stdio: ["ignore", "pipe", "pipe"] });
});
