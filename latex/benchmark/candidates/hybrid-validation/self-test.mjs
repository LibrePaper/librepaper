#!/usr/bin/env node
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
const here = fileURLToPath(new URL(".", import.meta.url));
const refs = join(here, "references"), candidate = mkdtempSync(join(tmpdir(), "hybrid-validation-"));
try {
  cpSync(refs, candidate, { recursive: true });
  execFileSync(process.execPath, [join(here, "verify.mjs"), "--candidate", candidate], { stdio: "ignore" });
  cpSync(join(candidate, "first/91-sorting-schemes.pdf"), join(candidate, "citation-addition/91-sorting-schemes.pdf"));
  cpSync(join(candidate, "first/91-sorting-schemes.bbl"), join(candidate, "citation-addition/91-sorting-schemes.bbl"));
  let rejected = false;
  try { execFileSync(process.execPath, [join(here, "verify.mjs"), "--candidate", candidate], { stdio: "ignore" }); }
  catch { rejected = true; }
  if (!rejected) throw Error("stale output unexpectedly passed");
  console.log("hybrid-validation self-test: positive pass and stale citation PDF/.bbl rejection pass");
} finally { rmSync(candidate, { recursive: true, force: true }); }
