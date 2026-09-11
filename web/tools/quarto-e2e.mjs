// Real Quarto bundle HTTP API workflow: publish source and complete result
// bundles through the API, then verify context selection, atomic replacement,
// and reader access all work over HTTP.
//
// The reader no longer paints a published bundle into its pane: a document
// shows either the Markdown preview or a live Quarto preview run through a
// paired local app, never a previously published bundle's cached figures,
// "Full artifact" frame, or
// freshness text ("Results may be outdated", "Showing saved results", etc.),
// and there is no Render settings/Saved results dialog in the reader UI to
// drive. Those UI checks have been removed from this script; what remains
// exercises the CLI's/server's bundle-publication API surface directly
// (design note: `librepaper quarto import`/bundle endpoints may still exist
// server-side even though the reader experience no longer surfaces them).
//
// No local Quarto installation or local pairing is used by this check.
// Usage: node web/tools/quarto-e2e.mjs [librepaper-binary]
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { browser, until, pause } from "./browser-driver.mjs";
import { contextFingerprint, contextId, parameterSha256, parseQuarto } from "../src/lib/engines/quarto.js";

const binary = resolve(process.argv[2] || process.env.LIBREPAPER_BINARY || "target/debug/librepaper");
const directory = mkdtempSync(join(tmpdir(), "librepaper-quarto-e2e-"));
const port = 22000 + Math.floor(Math.random() * 5000);
const debugPort = port + 1;
const base = `http://localhost:${port}`;
const source = [
  "---", "title: Quarto browser acceptance", "format: html", "---", "",
  "Opening prose remains visible while cached computation results are retained.", "",
  "::: {.callout-note #workflow}", "This callout is part of the draft.", ":::", "",
  "```{r}", "#| label: fig-main", "#| fig-cap: Saved figure", "x <- 1", "plot(x)", "```", "",
].join("\n");
const image = Uint8Array.from(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", "base64"));
const css = Buffer.from("body { background-image: url(figure.png); }");
const artifact = Buffer.from("<!doctype html><html><head><link rel=\"stylesheet\" href=\"style.css\"></head><body><h1>Full artifact</h1><img src=\"figure.png\" alt=\"Full saved figure\"><p>Immutable rendered output.</p></body></html>");
const replacementImage = Uint8Array.from(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64"));
const replacementArtifact = Buffer.from("<!doctype html><html><body><h1>Replacement artifact</h1><img src=\"replacement.png\" alt=\"Replacement saved figure\"><p>New immutable rendered output.</p></body></html>");
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
  await until("Quarto E2E server", async () => {
    if (server.exitCode !== null) throw new Error(`server exited: ${serverError}`);
    return (await fetch(`${base}/api/config`)).ok;
  }, 20000);
  // The public-publisher server accepts anonymous uploads, but still scopes
  // ownership to the signed visitor cookie it gives the browser shell. Use
  // that supported path instead of forging an account session (which would
  // require a catalogue account and a live session generation).
  const shell = await fetch(`${base}/`);
  assert.ok(shell.ok, `GET /: ${shell.status}`);
  const setCookie = shell.headers.get("set-cookie") || "";
  const visitor = setCookie.match(/(?:^|,\s*)librepaper_visitor=([^;]+)/)?.[1];
  assert.ok(visitor, "the shell issues a signed visitor cookie");

  // The real browser receives its own visitor identity, then that same
  // signed cookie is used for every publication below. This exercises the
  // normal anonymous owner path without forging an account session.
  tab = await browser("firefox", join(directory, "browser"), debugPort);
  await tab.navigate(base);
  await until("browser origin", () => tab.evaluate(`location.origin === ${JSON.stringify(base)}`));

  const createBody = JSON.stringify({ title: "Quarto browser acceptance", source_format: "quarto", source });
  const createdJson = await tab.evaluate(`(async () => { const r = await fetch("/api/documents", { method: "POST", headers: { "content-type": "application/json", "x-librepaper-client": "1" }, body: ${JSON.stringify(createBody)} }); return await r.text(); })()`);
  const created = JSON.parse(createdJson);
  const slug = created.slug;
  assert.ok(slug, "the API returns a Quarto document slug");
  const historyJson = await tab.evaluate(`(async () => await (await fetch(${JSON.stringify(`/api/documents/${slug}/history`)}, { headers: { "x-librepaper-client": "1" } })).text())()`);
  const history = JSON.parse(historyJson);
  const checkpoint = history.checkpoints.at(-1);
  assert.ok(checkpoint?.sha, "the source has a durable checkpoint");
  const parsed = parseQuarto(source, { path: "main.qmd" });
  const parametersSha256 = await parameterSha256({});
  const computationSha256 = await contextFingerprint(parsed, {
    main: "main.qmd", format: "html", parametersSha256,
  });
  const context = await contextId({ format: "html" });
  const manifest = {
    schema: "librepaper-quarto-bundle/v1",
    render_id: "render-quarto-browser-e2e",
    document_id: slug,
    source: {
      revision: checkpoint.sha,
      tree_sha256: checkpoint.tree_sha || checkpoint.sha,
      main: "main.qmd",
      verification: "working-tree-verified",
    },
    context: {
      id: context, fingerprint_version: 1, computation_sha256: computationSha256,
      format: "html", profiles: [], parameters_sha256: parametersSha256,
    },
    provenance: {
      kind: "managed-local-render", quarto_version: "e2e", collector_version: "e2e",
      policy: "project-defaults", computation: "refreshed", external_inputs: "unknown",
      started_at: new Date().toISOString(), completed_at: new Date().toISOString(),
    },
    artifact: { kind: "html", entrypoint: "index.html", sha256: digest(artifact), size: artifact.length, mime: "text/html" },
    cells: [{
      id: "main.qmd#fig-main", source_path: "main.qmd", label: "fig-main",
      source_sha256: await (async () => {
        const cell = parsed.cells[0];
        return digest(Buffer.from(`${cell.language}\0${cell.info}\0${cell.rawCode}`));
      })(), context_sha256: computationSha256, coverage: "captured",
      outputs: [{ ordinal: 0, kind: "image", asset: "figure.png", content_sha256: digest(image), caption: "Saved figure" },
        { ordinal: 1, kind: "text", text: "cached computation text" }],
    }],
    assets: [
      { path: "figure.png", sha256: digest(image), mime: "image/png", size: image.length },
      { path: "style.css", sha256: digest(css), mime: "text/css", size: css.length },
    ],
    coverage: { full_artifact: true, cell_outputs: "complete", diagnostics: [] },
  };
  const publishBody = {
    manifest,
    blobs: [
      { sha256: digest(artifact), mime: "text/html", data: base64(artifact) },
      { sha256: digest(image), mime: "image/png", data: base64(image) },
      { sha256: digest(css), mime: "text/css", data: base64(css) },
    ],
    select: true,
    expected_generation: 0,
  };
  // Publish through the browser so its HttpOnly visitor cookie is attached.
  const publishPath = JSON.stringify(`/api/documents/${slug}/quarto/bundles`);
  const publishJson = JSON.stringify(JSON.stringify(publishBody));
  const publishStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method: "POST", headers: { "content-type": "application/json", "x-librepaper-client": "1" }, body: ${publishJson} })).status)()`);
  assert.ok([200, 201].includes(publishStatus), `bundle publication failed: ${publishStatus}`);
  const shareUrl = created.share_url;
  assert.ok(shareUrl, "the document has a read link");

  const selectedPath = (forContext) => JSON.stringify(`/api/documents/${slug}/quarto/bundles/selected/${encodeURIComponent(forContext)}`);
  const fetchJson = async (path, extraHeaders = {}) => JSON.parse(await tab.evaluate(`(async () => await (await fetch(${path}, { headers: { "x-librepaper-client": "1", ...${JSON.stringify(extraHeaders)} } })).text())()`));

  const selected = await fetchJson(selectedPath(context));
  assert.equal(selected.manifest.render_id, manifest.render_id, "the selected bundle for this context is the one just published");

  // Distinct format/profile/parameter contexts are stored and selected
  // independently: publishing one must not change what another context
  // resolves to.
  const parameters = { seed: 42, enabled: true, label: "42", empty: null };
  const reviewContext = await contextId({ format: "html", profiles: ["review"], parameters });
  const reviewParametersSha = await parameterSha256(parameters);
  const reviewComputationSha = await contextFingerprint(parsed, {
    main: "main.qmd", format: "html", profiles: ["review"], parametersSha256: reviewParametersSha,
  });
  const review = structuredClone(publishBody);
  review.manifest.render_id = "review-profile-results";
  review.manifest.context = { ...review.manifest.context, id: reviewContext,
    profiles: ["review"], parameters_sha256: reviewParametersSha, computation_sha256: reviewComputationSha };
  review.manifest.cells[0].context_sha256 = reviewComputationSha;
  review.manifest.cells[0].outputs[1].text = "review profile computation";
  const reviewStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method: "POST", headers: { "content-type": "application/json", "x-librepaper-client": "1" }, body: ${JSON.stringify(JSON.stringify(review))} })).status)()`);
  assert.ok([200, 201].includes(reviewStatus), `review context publication failed: ${reviewStatus}`);

  const selectedReview = await fetchJson(selectedPath(reviewContext));
  assert.equal(selectedReview.manifest.render_id, review.manifest.render_id, "the review context resolves to its own bundle");
  const selectedDefaultAfterReview = await fetchJson(selectedPath(context));
  assert.equal(selectedDefaultAfterReview.manifest.render_id, manifest.render_id, "the default context is unaffected by publishing another context");

  // Result comments keep the immutable render identity, rather than silently
  // following a later replacement selected for the same context. Publish a
  // replacement with a different image and render id, using the expected
  // generation returned above, and confirm the compare-and-swap selection.
  const replacementManifest = JSON.parse(JSON.stringify(manifest));
  replacementManifest.render_id = "render-quarto-browser-replacement";
  replacementManifest.artifact = { kind: "html", entrypoint: "index.html", sha256: digest(replacementArtifact), size: replacementArtifact.length, mime: "text/html" };
  replacementManifest.cells[0].outputs[0] = { ordinal: 0, kind: "image", asset: "replacement.png", content_sha256: digest(replacementImage), caption: "Replacement saved figure" };
  replacementManifest.cells[0].outputs[1] = { ordinal: 1, kind: "text", text: "replacement computation text" };
  replacementManifest.assets = [{ path: "replacement.png", sha256: digest(replacementImage), mime: "image/png", size: replacementImage.length }];
  const replacementBody = {
    manifest: replacementManifest,
    blobs: [
      { sha256: digest(replacementArtifact), mime: "text/html", data: base64(replacementArtifact) },
      { sha256: digest(replacementImage), mime: "image/png", data: base64(replacementImage) },
    ],
    select: true,
    expected_generation: selected.generation,
  };
  const replacementJson = JSON.stringify(JSON.stringify(replacementBody));
  const replacementStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method: "POST", headers: { "content-type": "application/json", "x-librepaper-client": "1" }, body: ${replacementJson} })).status)()`);
  assert.ok([200, 201].includes(replacementStatus), `replacement publication failed: ${replacementStatus}`);
  const selectedReplacement = await fetchJson(selectedPath(context));
  assert.equal(selectedReplacement.manifest.render_id, replacementManifest.render_id, "the replacement becomes the selected render");

  // A stale expected_generation must be rejected rather than silently
  // overwriting the just-published replacement.
  const staleBody = { ...replacementBody, manifest: { ...replacementManifest, render_id: "render-quarto-browser-stale" }, expected_generation: selected.generation };
  const staleStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method: "POST", headers: { "content-type": "application/json", "x-librepaper-client": "1" }, body: ${JSON.stringify(JSON.stringify(staleBody))} })).status)()`);
  assert.ok(staleStatus >= 400, `a stale expected_generation must be rejected, got ${staleStatus}`);

  // The visitor identity is HttpOnly in production, so document.cookie cannot
  // turn the owner tab into a reader. Reopen the public link in a fresh
  // profile to verify the selected output for a genuinely read-only visitor.
  await tab.close();
  tab = await browser("firefox", join(directory, "reload-browser"), debugPort + 2);
  await tab.navigate(`${base}${shareUrl}`);
  const shareKey = new URL(`${base}${shareUrl}`).hash.match(/^#k=(.*)$/)?.[1] || "";
  const keyHeader = { "X-LibrePaper-Key": decodeURIComponent(shareKey) };
  const selectedAfterReload = JSON.parse(await tab.evaluate(`(async () => { const response = await fetch(${selectedPath(context)}, { headers: ${JSON.stringify(keyHeader)} }); return JSON.stringify({ status: response.status, body: await response.text() }); })()`));
  assert.equal(selectedAfterReload.status, 200, `selected bundle is readable after reload: ${selectedAfterReload.body}`);
  const selectedManifest = JSON.parse(selectedAfterReload.body).manifest;
  assert.equal(selectedManifest.context.id, context, "reloaded selection keeps the stable target context");
  assert.equal(selectedManifest.render_id, replacementManifest.render_id, "a read-only reader sees the same selected replacement");
  const artifactAfterReload = await tab.evaluate(`(async () => (await fetch(${JSON.stringify(`/api/documents/${slug}/quarto/bundles/${encodeURIComponent(selectedManifest.render_id)}/artifact`)}, { headers: ${JSON.stringify(keyHeader)} })).status)()`);
  assert.equal(artifactAfterReload, 200, `selected artifact is readable after reload: ${artifactAfterReload}`);
  const assetPaths = selectedManifest.assets.map((asset) => `/api/documents/${slug}/quarto/bundles/${encodeURIComponent(selectedManifest.render_id)}/asset?path=${encodeURIComponent(asset.path)}`);
  const assetStatuses = await tab.evaluate(`(async () => { const paths = ${JSON.stringify(assetPaths)}; const headers = ${JSON.stringify(keyHeader)}; return JSON.stringify(await Promise.all(paths.map(async (path) => (await fetch(path, { headers })).status))); })()`);
  assert.ok(JSON.parse(assetStatuses).every((status) => status === 200), `selected assets are readable after reload: ${assetStatuses}`);

  console.log("quarto-e2e: bundle publication, per-context selection, atomic replacement, and read-only reader access over HTTP passed");
} catch (error) {
  if (tab) console.error("quarto-e2e page:", await tab.evaluate("document.body.innerText.slice(-4000)").catch(() => "page unavailable"));
  throw error;
} finally {
  await tab?.close();
  server.kill();
  await Promise.race([new Promise((done) => server.once("exit", done)), pause(2000)]);
  rmSync(directory, { recursive: true, force: true });
}
