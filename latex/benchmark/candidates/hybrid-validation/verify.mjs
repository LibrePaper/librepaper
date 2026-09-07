#!/usr/bin/env node
// Verify a hybrid runtime's phase outputs against native references.
import { existsSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { EXAMPLE, MARKERS, PHASES, fixture } from "./fixture.mjs";
import { treeDigest } from "../../corpus.mjs";
import { inspectPDF } from "../../results.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const arg = name => { const i = process.argv.indexOf(name); return i < 0 ? null : process.argv[i + 1]; };
const candidate = resolve(arg("--candidate") || join(HERE, "candidate-output"));
const refs = resolve(arg("--references") || join(HERE, "references"));
const stem = basename(EXAMPLE.main, ".tex");
const normalize = s => s.normalize("NFKC").replace(/\s+/gu, " ").trim();
const digest = path => createHash("sha256").update(readFileSync(path)).digest("hex");
const failures = [], report = { candidate, references: refs, phases: [] };
let previousPdfDigest = null, previousBblDigest = null;
for (const phase of PHASES) {
  const dir = join(candidate, phase), refDir = join(refs, phase);
  const pdfPath = join(dir, stem + ".pdf"), bblPath = join(dir, stem + ".bbl");
  const refMetaPath = join(refDir, "reference.json"), refPdfPath = join(refDir, stem + ".pdf"), refBblPath = join(refDir, stem + ".bbl");
  const row = { phase, checks: {} };
  if (!existsSync(pdfPath)) failures.push(`${phase}: missing PDF`); else {
    const got = inspectPDF(pdfPath), ref = JSON.parse(readFileSync(refMetaPath, "utf8"));
    row.checks.tree = ref.treeSha256 === treeDigest(fixture(phase));
    if (!row.checks.tree) failures.push(`${phase}: native reference is stale for fixture tree`);
    row.checks.pdf = got.pdf; row.checks.pages = got.pages === ref.pages; row.checks.text = normalize(got.text) === normalize(ref.text); row.checks.marker = normalize(got.text).includes(normalize(MARKERS[phase]));
    if (!row.checks.pdf) failures.push(`${phase}: invalid PDF`);
    if (!row.checks.pages) failures.push(`${phase}: page count ${got.pages} != ${ref.pages}`);
    if (!row.checks.text) failures.push(`${phase}: normalized PDF text differs from native`);
    if (!row.checks.marker) failures.push(`${phase}: exact Unicode marker missing`);
    const current = digest(pdfPath);
    if (previousPdfDigest && current === previousPdfDigest) failures.push(`${phase}: PDF is byte-identical to prior phase; mutation may be stale`);
    previousPdfDigest = current;
  }
  if (!existsSync(bblPath) || !existsSync(refBblPath)) failures.push(`${phase}: missing .bbl`); else {
    row.checks.bbl = readFileSync(bblPath).equals(readFileSync(refBblPath));
    if (!row.checks.bbl) failures.push(`${phase}: .bbl differs byte-for-byte from native`);
    const current = digest(bblPath);
    if (previousBblDigest && (phase === "citation-addition" || phase === "title-edit") && current === previousBblDigest) failures.push(`${phase}: .bbl unchanged after bibliography mutation`);
    previousBblDigest = current;
  }
  report.phases.push(row);
}
// These checks guard the mutations themselves, independent of whitespace wrapping.
if (existsSync(join(candidate, "citation-addition", stem + ".pdf"))) {
  const text = inspectPDF(join(candidate, "citation-addition", stem + ".pdf")).text;
  if (!/Glashow/.test(text) || !/Partial Symmetries of Weak Interactions/.test(text)) failures.push("citation-addition: glashow citation/bibliography row not rendered");
}
if (existsSync(join(candidate, "title-edit", stem + ".pdf")) && !/HYBRID\s+title\s+edit/.test(inspectPDF(join(candidate, "title-edit", stem + ".pdf")).text)) failures.push("title-edit: edited companion title not rendered");
report.ok = failures.length === 0; report.failures = failures;
console.log(JSON.stringify(report, null, 2));
process.exitCode = report.ok ? 0 : 1;
