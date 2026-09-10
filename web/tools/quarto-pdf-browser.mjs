// Quarto PDF reader acceptance: publish a real PDF artifact, then make the
// Reader display it through its normal PDF preview path. This deliberately
// uses no local Quarto installation; the bytes are a small valid PDF fixture.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { browser, until, pause } from "./browser-driver.mjs";
import { contextFingerprint, contextId, parameterSha256, parseQuarto } from "../src/lib/quarto.js";

const binary = resolve(process.argv[2] || process.env.LIBREPAPER_BINARY || "target/debug/librepaper");
const directory = mkdtempSync(join(tmpdir(), "librepaper-quarto-pdf-e2e-"));
const port = 25000 + Math.floor(Math.random() * 3000);
const debugPort = port + 1;
const base = `http://localhost:${port}`;

function realPdf() {
  const stream = "BT\n/F1 18 Tf\n20 100 Td\n(Quarto PDF fixture) Tj\nET\n";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}endstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  let bytes = "%PDF-1.4\n";
  const offsets = [0];
  for (const [index, object] of objects.entries()) {
    offsets.push(Buffer.byteLength(bytes));
    bytes += `${index + 1} 0 obj\n${object}\nendobj\n`;
  }
  const xref = Buffer.byteLength(bytes);
  bytes += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets.slice(1)) bytes += `${String(offset).padStart(10, "0")} 00000 n \n`;
  bytes += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(bytes, "binary");
}

const source = "---\ntitle: Quarto PDF browser acceptance\nformat: pdf\n---\n\nA stored Quarto PDF is readable.\n";
const artifact = realPdf();
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const base64 = (bytes) => Buffer.from(bytes).toString("base64");
const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LIBREPAPER_")));
const server = spawn(binary, ["serve", "--port", String(port), "--data", join(directory, "data"), "--publishers", "anyone", "--commenters", "anyone"], {
  stdio: ["ignore", "ignore", "pipe"], env: environment,
});
let serverError = "";
server.stderr.on("data", (bytes) => { serverError += String(bytes); });
let tab;
try {
  await until("Quarto PDF server", async () => {
    if (server.exitCode !== null) throw new Error(`server exited: ${serverError}`);
    return (await fetch(`${base}/api/config`)).ok;
  }, 20000);
  tab = await browser("firefox", join(directory, "browser"), debugPort);
  await tab.navigate(base);
  await until("browser origin", () => tab.evaluate(`location.origin === ${JSON.stringify(base)}`));
  const createBody = JSON.stringify({ title: "Quarto PDF browser acceptance", source_format: "quarto", source });
  const created = JSON.parse(await tab.evaluate(`(async () => await (await fetch("/api/documents", { method:"POST", headers:{"content-type":"application/json","x-librepaper-client":"1"}, body:${JSON.stringify(createBody)} })).text())()`));
  assert.ok(created.slug, "the API returns a Quarto PDF document slug");
  const history = JSON.parse(await tab.evaluate(`(async () => await (await fetch(${JSON.stringify(`/api/documents/${created.slug}/history`)}, { headers:{"x-librepaper-client":"1"} })).text())()`));
  const checkpoint = history.checkpoints.at(-1);
  assert.ok(checkpoint?.sha, "the PDF source has a durable checkpoint");
  const parsed = parseQuarto(source, { path:"main.qmd" });
  const parametersSha256 = await parameterSha256({});
  const computationSha256 = await contextFingerprint(parsed, { main:"main.qmd", format:"pdf", parametersSha256 });
  const context = await contextId({ format:"pdf" });
  const manifest = {
    schema:"librepaper-quarto-bundle/v1", render_id:"render-quarto-pdf-browser-e2e", document_id:created.slug,
    source:{ revision:checkpoint.sha, tree_sha256:checkpoint.tree_sha || checkpoint.sha, main:"main.qmd", verification:"working-tree-verified" },
    context:{ id:context, fingerprint_version:1, computation_sha256:computationSha256, format:"pdf", profiles:[], parameters_sha256:parametersSha256 },
    provenance:{ kind:"managed-local-render", quarto_version:"e2e", collector_version:"e2e", policy:"project-defaults", computation:"refreshed", external_inputs:"unknown", started_at:new Date().toISOString(), completed_at:new Date().toISOString() },
    artifact:{ kind:"pdf", entrypoint:"report.pdf", sha256:digest(artifact), size:artifact.length, mime:"application/pdf" },
    cells:[], assets:[], coverage:{ full_artifact:true, cell_outputs:"none", diagnostics:[] },
  };
  const publish = { manifest, blobs:[{ sha256:digest(artifact), mime:"application/pdf", data:base64(artifact) }], select:true, expected_generation:0 };
  const status = await tab.evaluate(`(async () => (await fetch(${JSON.stringify(`/api/documents/${created.slug}/quarto/bundles`)}, { method:"POST", headers:{"content-type":"application/json","x-librepaper-client":"1"}, body:${JSON.stringify(JSON.stringify(publish))} })).status)()`);
  assert.ok([200, 201].includes(status), `PDF bundle publication failed: ${status}`);
  await tab.navigate(`${base}/docs/${created.slug}`);
  await until("Quarto PDF editor", () => tab.evaluate('!!document.querySelector(".cm-content")'));
  await tab.evaluate('([...document.querySelectorAll("button")].find((button) => button.textContent.includes("Quarto output")) || {}).click?.()');
  await until("Quarto PDF frame", () => tab.evaluate('document.querySelector("iframe")?.src.includes("/pdf/")'));
  await until("Quarto PDF text", async () => (await tab.text()).includes("Quarto PDF fixture"));
  assert.match(await tab.evaluate('document.querySelector("iframe").src'), /\/pdf\//);
  console.log("quarto-pdf-browser: real PDF artifact loaded through the Quarto Reader preview");
} catch (error) {
  if (tab) console.error("quarto-pdf-browser page:", await tab.evaluate("document.body.innerText.slice(-2500)").catch(() => "page unavailable"));
  throw error;
} finally {
  await tab?.close();
  server.kill();
  await Promise.race([new Promise((done) => server.once("exit", done)), pause(2000)]);
  rmSync(directory, { recursive:true, force:true });
}
