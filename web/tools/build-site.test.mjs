import assert from "node:assert/strict";
import { mkdtemp, mkdir, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { prepareSiteOutput } from "./site-output.mjs";

test("a new site build drops pages from the preceding build", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "librepaper-site-output-"));
  const output = join(temporary, ".build");
  try {
    await mkdir(join(output, "old-section"), { recursive: true });
    await writeFile(join(output, "old-section", "removed.html"), "stale page");

    await prepareSiteOutput(output);

    assert.deepEqual(await readdir(output), []);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});
