// Loro ships the Rust crate and the wasm/npm build from one repository at one
// version. loro-crdt carries a patch number the crate does not, so agreement
// is on major.minor -- a divergence there means the two sides are speaking
// different wire formats, which is the whole failure this guards.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const cargoToml = path.join(root, "Cargo.toml");
const packageJson = path.join(root, "web/package.json");

function extractVersion(filePath, pattern) {
  const content = readFileSync(filePath, "utf8");
  const match = content.match(pattern);
  if (!match || !match[1]) {
    return null;
  }
  return match[1];
}

function extractMajorMinor(version) {
  const parts = version.split(".");
  return `${parts[0]}.${parts[1]}`;
}

test("loro and loro-crdt major.minor versions must agree", () => {
  const rustVersion = extractVersion(cargoToml, /loro = "([0-9.]+)"/);
  assert(rustVersion !== null, "no loro version in Cargo.toml");

  const npmVersion = extractVersion(packageJson, /"loro-crdt": "([0-9.]+)"/);
  assert(npmVersion !== null, "no loro-crdt version in web/package.json");

  const rustMajorMinor = extractMajorMinor(rustVersion);
  const npmMajorMinor = extractMajorMinor(npmVersion);

  assert.equal(
    rustMajorMinor,
    npmMajorMinor,
    `loro ${rustVersion} (rust) and loro-crdt ${npmVersion} (npm) disagree on major.minor versions; the wire format is shared and these must move together`,
  );
});
