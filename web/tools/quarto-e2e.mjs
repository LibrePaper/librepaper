// Real Quarto reader workflow: publish source and a complete result bundle
// through the HTTP API, then exercise Draft/full-output loading in the browser.
// No local Quarto installation or local pairing is used by this check.
// Usage: node web/tools/quarto-e2e.mjs [librepaper-binary]
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { browser, until, pause } from "./browser-driver.mjs";
import { contextFingerprint, contextId, parameterSha256, parseQuarto } from "../src/lib/quarto.js";

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
const replacementImage = Uint8Array.from(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64"));
const css = Buffer.from("body { background-image: url(figure.png); }");
const artifact = Buffer.from("<!doctype html><html><head><link rel=\"stylesheet\" href=\"style.css\"></head><body><h1>Full artifact</h1><img src=\"figure.png\" alt=\"Full saved figure\"><p>Immutable rendered output.</p></body></html>");
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
  const cookie = `librepaper_visitor=${visitor}`;
  const headers = { "content-type": "application/json", "x-librepaper-client": "1", cookie };
  const request = async (path, method = "GET", body, extra = {}) => {
    const response = await fetch(`${base}${path}`, { method, headers: { ...headers, ...extra }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    const result = await response.json().catch(() => ({}));
    assert.ok(response.ok, `${method} ${path}: ${response.status} ${JSON.stringify(result)}`);
    return result;
  };

  // Let the real browser receive its own visitor identity first, then use
  // that same signed cookie for the API publication. This exercises the
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
  // Publish through the browser so its HttpOnly visitor cookie is attached;
  // that is the same path a real Reader uses when sharing a local render.
  const publishPath = JSON.stringify(`/api/documents/${slug}/quarto/bundles`);
  const publishJson = JSON.stringify(JSON.stringify(publishBody));
  const publishStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method: "POST", headers: { "content-type": "application/json", "x-librepaper-client": "1" }, body: ${publishJson} })).status)()`);
  assert.ok([200, 201].includes(publishStatus), `bundle publication failed: ${publishStatus}`);
  const shareUrl = created.share_url;
  assert.ok(shareUrl, "the document has a read link");

  await tab.navigate(base);
  await until("browser origin", () => tab.evaluate(`location.origin === ${JSON.stringify(base)}`));
  const apiPath = JSON.stringify(`/api/documents/${slug}`);
  const browserStatus = await tab.evaluate(`(async () => (await fetch(${apiPath}, { headers: { "x-librepaper-client": "1" } })).status)()`);
  assert.equal(browserStatus, 200, `browser document access failed: ${browserStatus}`);
  await tab.navigate(`${base}/docs/${slug}`);
  await until("reader origin", () => tab.evaluate(`location.pathname === ${JSON.stringify(`/docs/${slug}`)}`));
  const frameText = async () => (await tab.frameEvaluate("document.body.innerText", 2)) || (await tab.frameEvaluate("document.body.innerText")) || "";
  await until("Quarto editor", () => tab.evaluate('!!document.querySelector(".cm-content")'));
  await until("cached draft text", async () => (await frameText()).includes("cached computation text"));
  await until("draft callout", async () => (await frameText()).includes("This callout is part of the draft."));
  await until("draft cached image", async () => tab.frameEvaluate('Boolean(document.querySelector("img[src^=\\"blob:\\"]"))'));
  assert.ok(await tab.frameEvaluate('Boolean(document.querySelector("figure.quarto-cached-output"))'), "Draft renders the cached figure");

  // Context controls select saved computations without a local execution
  // connection, and keep types/profile identity across a browser reload.
  const parameters = { seed:42, enabled:true, label:"42", empty:null };
  const reviewContext = await contextId({ format:"html", profiles:["review"], parameters });
  const reviewParametersSha = await parameterSha256(parameters);
  const reviewComputationSha = await contextFingerprint(parsed, {
    main:"main.qmd", format:"html", profiles:["review"], parametersSha256:reviewParametersSha,
  });
  const review = structuredClone(publishBody);
  review.manifest.render_id = "review-profile-results";
  review.manifest.context = { ...review.manifest.context, id:reviewContext,
    profiles:["review"], parameters_sha256:reviewParametersSha, computation_sha256:reviewComputationSha };
  review.manifest.cells[0].context_sha256 = reviewComputationSha;
  review.manifest.cells[0].outputs[1].text = "review profile computation";
  const reviewStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method:"POST", headers:{ "content-type":"application/json", "x-librepaper-client":"1" }, body:${JSON.stringify(JSON.stringify(review))} })).status)()`);
  assert.ok([200,201].includes(reviewStatus), `review context publication failed: ${reviewStatus}`);
  const slides = structuredClone(review);
  slides.manifest.render_id = "review-slides-results";
  slides.manifest.context.format = "revealjs";
  slides.manifest.context.id = await contextId({ format:"revealjs", profiles:["review"], parameters });
  slides.manifest.context.computation_sha256 = await contextFingerprint(parsed, {
    main:"main.qmd", format:"revealjs", profiles:["review"], parametersSha256:reviewParametersSha,
  });
  slides.manifest.cells[0].context_sha256 = slides.manifest.context.computation_sha256;
  slides.manifest.cells[0].outputs[1].text = "review slides computation";
  const slidesStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method:"POST", headers:{ "content-type":"application/json", "x-librepaper-client":"1" }, body:${JSON.stringify(JSON.stringify(slides))} })).status)()`);
  assert.ok([200,201].includes(slidesStatus), `slides context publication failed: ${slidesStatus}`);

  const editOptions = async (profile, values, format = "default") => {
    await tab.evaluate(`(() => {
      document.querySelector("details.render-options").open = true;
      const fields = [
        ['[aria-label="Quarto profile (optional)"]', ${JSON.stringify(profile)}, 'input'],
        ['[aria-label="Quarto parameters as JSON"]', ${JSON.stringify(values)}, 'input'],
        ['[aria-label="Render format"]', ${JSON.stringify(format)}, 'change'],
      ];
      for (const [selector,value,type] of fields) {
        const field = document.querySelector(selector);
        if (!field) throw new Error('Missing render option: '+selector);
        field.value = value;
        field.dispatchEvent(new Event(type,{bubbles:true}));
      }
    })()`);
  };
  const applyOptions = async () => {
    await until("render options valid", () => tab.evaluate('!![...document.querySelectorAll("button")].find(b => b.textContent.trim() === "Apply options" && !b.disabled)'));
    await tab.evaluate('[...document.querySelectorAll("button")].find(b => b.textContent.trim() === "Apply options").click()');
  };
  const openCollaborationComments = async () => {
    await tab.evaluate('(() => { const button = document.querySelector(\'button[aria-label="Collaboration"]\'); if (button?.getAttribute("aria-pressed") !== "true") button?.click(); })()');
    await until("collaboration panel", () => tab.evaluate('Boolean(document.querySelector("#collaboration-tab-comments"))'));
    await tab.evaluate('document.querySelector("#collaboration-tab-comments")?.click()');
  };
  await editOptions("review", JSON.stringify(parameters));
  await applyOptions();
  await until("profile-specific cached output", async () => (await frameText()).includes("review profile computation"));
  assert.ok(!(await frameText()).includes("cached computation text"), "different context must not borrow default results");
  await tab.navigate(`${base}/docs/${slug}`);
  await until("remembered profile output", async () => (await frameText()).includes("review profile computation"));
  assert.equal(await tab.evaluate('document.querySelector(\'[aria-label="Quarto profile (optional)"]\').value'), "review");
  assert.deepEqual(JSON.parse(await tab.evaluate('document.querySelector(\'[aria-label="Quarto parameters as JSON"]\').value')), parameters);
  await editOptions("review", '{"nested":{"value":1}}');
  await until("invalid parameters diagnostic", () => tab.evaluate('Boolean(document.querySelector(".render-options-error"))'));
  assert.ok((await frameText()).includes("review profile computation"), "invalid edits do not change the applied context");
  await editOptions("review", JSON.stringify(parameters), "revealjs");
  await applyOptions();
  await until("explicit format context", async () => (await frameText()).includes("review slides computation"));
  await editOptions("missing-profile", "{}");
  await applyOptions();
  await until("missing context clears cached outputs", async () => {
    const text = await frameText();
    return text.includes("Opening prose") && !text.includes("review profile computation") && !text.includes("review slides computation") && !text.includes("cached computation text");
  });
  await editOptions("", "{}");
  await applyOptions();
  await until("default context restored", async () => (await frameText()).includes("cached computation text"));
  await tab.evaluate('document.querySelector("details.render-options").open = false');

  await tab.evaluate('window.__quartoEditErrors = []; window.addEventListener("error", (event) => window.__quartoEditErrors.push(event.message))');
  const proseEdit = source.replace("Opening prose remains visible", "Edited prose remains visible");
  await tab.insert(proseEdit, true);
  await pause(1200);
  const editedSource = await tab.evaluate("Array.from(document.querySelectorAll('.cm-content .cm-line')).map((line) => line.textContent || '').join('\\n')");
  assert.equal(editedSource.replace(/\\r\\n/g, "\\n"), proseEdit, "the browser editor round-trips the complete prose edit");
  await until("prose edit", async () => (await frameText()).includes("Edited prose remains visible"));
  await until("cached result after prose edit", async () => (await frameText()).includes("cached computation text"));
  assert.deepEqual(await tab.evaluate("window.__quartoEditErrors"), [], "editing must not dispatch diagnostics inside a CodeMirror update");
  assert.match(await tab.evaluate("document.body.innerText"), /Saved results; computation source unchanged|Showing saved results/);

  const codeEdit = proseEdit.replace("x <- 1", "x <- 2");
  await tab.insert(codeEdit, true);
  const editedCodeSource = await tab.evaluate("Array.from(document.querySelectorAll('.cm-content .cm-line')).map((line) => line.textContent || '').join('\\n')");
  assert.equal(editedCodeSource.replace(/\\r\\n/g, "\\n"), codeEdit, "the browser editor round-trips the upstream code edit");
  await until("upstream edit", async () => (await tab.evaluate("document.body.innerText")).includes("Results may be outdated"));
  assert.ok((await tab.text()).includes("cached computation text"), "stale cached output remains visible");

  await tab.evaluate('(async () => { [...document.querySelectorAll(".menubar-item")].find((el) => el.textContent.trim() === "Tools")?.click(); await new Promise((resolve) => setTimeout(resolve, 80)); const item = [...document.querySelectorAll(\'[role="menuitem"]\')].find((el) => el.textContent.includes("Show rendered output")); item?.dispatchEvent(new PointerEvent("pointermove", { pointerType:"mouse", bubbles:true })); item?.click(); })()');
  await until("full Quarto output", async () => (await frameText()).includes("Full artifact"));
  assert.ok(await tab.frameEvaluate('document.querySelector("img").src.startsWith("data:")', 2), "full artifact image uses an opaque data URL");
  assert.ok(await tab.frameEvaluate('document.querySelector("link").href.startsWith("data:")', 2), "full artifact stylesheet uses an opaque data URL");

  // Result comments keep the immutable render identity, rather than silently
  // following a later replacement selected for the same context. Exercise
  // the complete owner path: open saved results, comment on the image, then
  // publish a replacement with a different image and render id.
  await tab.evaluate('([...document.querySelectorAll("button")].find((button) => button.textContent.includes("Saved results")) || {}).click?.()');
  await until("saved results dialog", () => tab.evaluate('document.body.innerText.includes("Comment on this result")'));
  await tab.evaluate('([...document.querySelectorAll("button")].find((button) => button.textContent.includes("Comment on this result")) || {}).click?.()');
  await until("result comment dialog", () => tab.evaluate('Boolean(document.querySelector("#commentForm"))'));
  await tab.evaluate(`(() => {
    const field = document.querySelector("#commentForm textarea");
    field.value = "Original result comment";
    field.dispatchEvent(new Event("input", { bubbles:true }));
  })()`);
  await tab.evaluate('document.querySelector("button[type=submit][form=commentForm]")?.click()');
  await openCollaborationComments();
  await until("saved result comment", () => tab.evaluate('document.body.innerText.includes("Original result comment")'));

  const replacementArtifact = Buffer.from("<!doctype html><html><body><h1>Replacement artifact</h1><img src=\"replacement.png\" alt=\"Replacement saved figure\"><p>New immutable rendered output.</p></body></html>");
  const replacementManifest = JSON.parse(JSON.stringify(manifest));
  replacementManifest.render_id = "render-quarto-browser-replacement";
  replacementManifest.artifact = { kind:"html", entrypoint:"index.html", sha256:digest(replacementArtifact), size:replacementArtifact.length, mime:"text/html" };
  replacementManifest.cells[0].outputs[0] = { ordinal:0, kind:"image", asset:"replacement.png", content_sha256:digest(replacementImage), caption:"Replacement saved figure" };
  replacementManifest.cells[0].outputs[1] = { ordinal:1, kind:"text", text:"replacement computation text" };
  replacementManifest.assets = [{ path:"replacement.png", sha256:digest(replacementImage), mime:"image/png", size:replacementImage.length }];
  const selectedBeforeReplacement = JSON.parse(await tab.evaluate(`(async () => await (await fetch(${JSON.stringify(`/api/documents/${slug}/quarto/bundles/selected/${encodeURIComponent(context)}`)}, { headers: { "x-librepaper-client":"1" } })).text())()`));
  const replacementBody = {
    manifest: replacementManifest,
    blobs: [
      { sha256:digest(replacementArtifact), mime:"text/html", data:base64(replacementArtifact) },
      { sha256:digest(replacementImage), mime:"image/png", data:base64(replacementImage) },
    ],
    select:true,
    expected_generation:selectedBeforeReplacement.generation,
  };
  const replacementJson = JSON.stringify(JSON.stringify(replacementBody));
  const replacementStatus = await tab.evaluate(`(async () => (await fetch(${publishPath}, { method:"POST", headers:{ "content-type":"application/json", "x-librepaper-client":"1" }, body:${replacementJson} })).status)()`);
  assert.ok([200, 201].includes(replacementStatus), `replacement publication failed: ${replacementStatus}`);
  const selectedReplacement = JSON.parse(await tab.evaluate(`(async () => await (await fetch(${JSON.stringify(`/api/documents/${slug}/quarto/bundles/selected/${encodeURIComponent(context)}`)}, { headers: { "x-librepaper-client":"1" } })).text())()`));
  assert.equal(selectedReplacement.manifest.render_id, replacementManifest.render_id, "the replacement becomes the selected render");

  // A fresh page must still resolve the comment's old render id and retain its
  // original image, even though the context now selects the replacement.
  await tab.navigate(`${base}/docs/${slug}`);
  await until("reloaded Quarto editor", () => tab.evaluate('!!document.querySelector(".cm-content")'));
  await tab.evaluate('([...document.querySelectorAll("button")].find((button) => button.textContent.includes("Saved results")) || {}).click?.()');
  await until("replacement saved result", () => tab.evaluate('document.body.innerText.includes("Replacement saved figure")'));
  assert.ok(!(await tab.evaluate('document.body.innerText.includes("Original saved figure")')), "the selected results modal follows the replacement render");
  // Reopening the route closes the modal through the normal lifecycle before
  // the comment card is inspected, and also exercises the persisted selection
  // one more time after the result view has been loaded.
  await tab.navigate(`${base}/docs/${slug}`);
  await until("comment inspection reload", () => tab.evaluate('!!document.querySelector(".cm-content")'));
  await openCollaborationComments();
  await until("reloaded result comment", () => tab.evaluate('document.body.innerText.includes("Original result comment")'));
  await until("original result inspection control", () => tab.evaluate('(() => { const button = [...document.querySelectorAll("button")].find((candidate) => candidate.textContent.includes("Inspect original result")); if (!button) return false; button.click(); return true; })()'));
  await until("original result dialog", () => tab.evaluate('document.body.innerText.includes("Original saved result") && document.body.innerText.includes("render-quarto-browser-e2e")'));
  assert.ok(await tab.evaluate('document.body.innerText.includes("Saved figure")'), "inspection shows the original result caption after reload");
  assert.ok(!(await tab.evaluate('document.body.innerText.includes("Replacement saved figure")')), "inspection does not substitute the newer result");

  // The visitor identity is HttpOnly in production, so document.cookie cannot
  // turn the owner tab into a reader. Reopen the public link in a fresh
  // profile to verify the selected output for a genuinely read-only visitor.
  await tab.close();
  tab = await browser("firefox", join(directory, "reload-browser"), debugPort + 2);
  await tab.navigate(`${base}${shareUrl}`);
  const shareKey = new URL(`${base}${shareUrl}`).hash.match(/^#k=(.*)$/)?.[1] || "";
  const keyHeader = { "X-LibrePaper-Key": decodeURIComponent(shareKey) };
  const selectedAfterReload = JSON.parse(await tab.evaluate(`(async () => { const response = await fetch(${JSON.stringify(`/api/documents/${slug}/quarto/bundles/selected/${encodeURIComponent(context)}`)}, { headers: ${JSON.stringify(keyHeader)} }); return JSON.stringify({ status: response.status, body: await response.text() }); })()`));
  assert.equal(selectedAfterReload.status, 200, `selected bundle is readable after reload: ${selectedAfterReload.body}`);
  const selectedManifest = JSON.parse(selectedAfterReload.body).manifest;
  assert.equal(selectedManifest.context.id, context, "reloaded selection keeps the stable target context");
  const artifactAfterReload = await tab.evaluate(`(async () => (await fetch(${JSON.stringify(`/api/documents/${slug}/quarto/bundles/${encodeURIComponent(selectedManifest.render_id)}/artifact`)}, { headers: ${JSON.stringify(keyHeader)} })).status)()`);
  assert.equal(artifactAfterReload, 200, `selected artifact is readable after reload: ${artifactAfterReload}`);
  const assetPaths = selectedManifest.assets.map((asset) => `/api/documents/${slug}/quarto/bundles/${encodeURIComponent(selectedManifest.render_id)}/asset?path=${encodeURIComponent(asset.path)}`);
  const assetStatuses = await tab.evaluate(`(async () => { const paths = ${JSON.stringify(assetPaths)}; const headers = ${JSON.stringify(keyHeader)}; return JSON.stringify(await Promise.all(paths.map(async (path) => (await fetch(path, { headers })).status))); })()`);
  assert.ok(JSON.parse(assetStatuses).every((status) => status === 200), `selected assets are readable after reload: ${assetStatuses}`);
  await until("selected bundle reload", async () => (await frameText()).includes("Replacement artifact"));
  assert.ok((await frameText()).includes("New immutable rendered output."), "reader reload fetches the selected replacement artifact");
  console.log("quarto-e2e: format/profile/parameter contexts, reload preferences, cached outputs, freshness, artifacts, and comments passed");
} catch (error) {
  if (tab) console.error("quarto-e2e page:", await tab.evaluate("document.body.innerText.slice(-4000)").catch(() => "page unavailable"));
  throw error;
} finally {
  await tab?.close();
  server.kill();
  await Promise.race([new Promise((done) => server.once("exit", done)), pause(2000)]);
  rmSync(directory, { recursive: true, force: true });
}
