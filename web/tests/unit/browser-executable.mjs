import assert from "node:assert/strict";
import { accessSync, chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { requireChromiumExecutable, resolveChromiumExecutable } from "../../tools/browser-executable.mjs";

const directory = mkdtempSync(join(tmpdir(), "librepaper-browser-binary-"));
function binary(name) {
  const path = join(directory, name);
  writeFileSync(path, "#!/bin/sh\nexit 0\n");
  chmodSync(path, 0o755);
  return path;
}

try {
  const chromium = binary("chromium");
  const chrome = binary("custom-chrome");
  const path = directory;

  assert.equal(resolveChromiumExecutable({ PATH: path }), chromium,
    "the standard Chromium name is discovered first");
  assert.equal(resolveChromiumExecutable({ PATH: path, LIBREPAPER_CHROMIUM: chrome }), chrome,
    "the LibrePaper explicit executable overrides PATH candidates");
  assert.equal(resolveChromiumExecutable({ PATH: path, CHROME_BIN: chrome }), chrome,
    "the conventional Chrome binary override is supported");
  assert.equal(resolveChromiumExecutable({ PATH: path, CHROMIUM_BIN: chrome }), chrome,
    "the Chromium-specific binary override is supported");
  assert.equal(resolveChromiumExecutable({ PATH: "" }), null,
    "an empty candidate path returns no executable");
  assert.throws(() => requireChromiumExecutable({ PATH: "" }), /No Chromium executable found/,
    "a required browser suite cannot turn a missing binary into a skip");
  assert.throws(() => resolveChromiumExecutable({ PATH: path, LIBREPAPER_CHROMIUM: "missing-chrome" }),
    /LIBREPAPER_CHROMIUM does not name an executable/,
    "a bad explicit path fails instead of silently selecting another binary");

  const originalDirectory = process.cwd();
  const workingDirectory = join(directory, "working-directory");
  const relativeBin = join(workingDirectory, "bin");
  mkdirSync(relativeBin);
  const relativeChromium = join(relativeBin, "chromium");
  writeFileSync(relativeChromium, "#!/bin/sh\nexit 0\n");
  chmodSync(relativeChromium, 0o755);
  try {
    process.chdir(workingDirectory);
    const resolved = resolveChromiumExecutable({ PATH: "./bin" });
    process.chdir(directory);
    assert.equal(resolved, relativeChromium,
      "relative PATH entries resolve to an absolute executable before the suite changes directories");
    accessSync(resolved);
  } finally {
    process.chdir(originalDirectory);
  }

  const alternateDirectory = mkdtempSync(join(tmpdir(), "librepaper-browser-alt-"));
  try {
    const alternate = join(alternateDirectory, "chromium-browser");
    writeFileSync(alternate, "#!/bin/sh\nexit 0\n");
    chmodSync(alternate, 0o755);
    assert.equal(resolveChromiumExecutable({ PATH: alternateDirectory }), alternate,
      "chromium-browser is a supported PATH candidate");
  } finally {
    rmSync(alternateDirectory, { recursive: true, force: true });
  }
  const googleDirectory = mkdtempSync(join(tmpdir(), "librepaper-browser-google-"));
  try {
    const google = join(googleDirectory, "google-chrome");
    writeFileSync(google, "#!/bin/sh\nexit 0\n");
    chmodSync(google, 0o755);
    assert.equal(resolveChromiumExecutable({ PATH: googleDirectory }), google,
      "google-chrome is a supported PATH candidate");
  } finally {
    rmSync(googleDirectory, { recursive: true, force: true });
  }
} finally {
  rmSync(directory, { recursive: true, force: true });
}

console.log("browser executable: explicit overrides, candidate order and missing-path behavior checked");
