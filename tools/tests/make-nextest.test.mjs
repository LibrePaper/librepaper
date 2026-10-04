import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const makefile = join(root, "Makefile");
const make = execFileSync("/bin/sh", ["-c", "command -v make"], { encoding: "utf8" }).trim();

async function runnerFixture(withNextest = false) {
  const directory = await mkdtemp(join(tmpdir(), "librepaper-nextest-"));
  const log = join(directory, "calls.log");
  await writeFile(
    join(directory, "cargo"),
    '#!/bin/sh\nif [ "$1" = nextest ]; then\n  shift\n  printf "nextest %s\\n" "$*" >> "$MOCK_LOG"\n  exit "${MOCK_NEXTEST_EXIT:-0}"\nfi\nprintf "cargo %s\\n" "$*" >> "$MOCK_LOG"\nexit "${MOCK_CARGO_EXIT:-0}"\n',
    { mode: 0o755 },
  );
  if (withNextest) {
    await writeFile(
      join(directory, "cargo-nextest"),
      "#!/bin/sh\nexit 0\n",
      { mode: 0o755 },
    );
  }
  return { directory, log };
}

function runTarget(directory, log, values = {}) {
  return execFileSync(make, [
    "--no-print-directory",
    "-f",
    makefile,
    "SHELL=/bin/sh",
    "WEB=",
    "SOURCES=",
    "test-rust",
  ], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      PATH: directory,
      MOCK_LOG: log,
      MAKEFLAGS: "",
      MFLAGS: "",
      MAKELEVEL: "",
      ...values,
    },
  });
}

test("test-rust falls back only when cargo-nextest is unavailable", async () => {
  const { directory, log } = await runnerFixture();
  try {
    const output = runTarget(directory, log);
    assert.match(output, /cargo-nextest not installed/);
    assert.equal(await readFile(log, "utf8"), "cargo test --workspace\n");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("test-rust preserves an installed cargo-nextest failure", async () => {
  const { directory, log } = await runnerFixture(true);
  try {
    assert.throws(() => runTarget(directory, log, { MOCK_NEXTEST_EXIT: "19" }));
    const calls = await readFile(log, "utf8");
    assert.equal(calls, "nextest run --workspace\n");
    assert.doesNotMatch(calls, /cargo test/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
