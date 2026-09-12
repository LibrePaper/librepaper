import assert from "node:assert/strict";
import { createPublicationPublisher, createPublicationReader, digestPublication, sha256 } from "../../src/lib/publication.js";

const bytes = new TextEncoder().encode("hello");
const bundle = await digestPublication({ html: "<p>hello</p>", assets: [{ path: "public/a.txt", bytes, mime: "text/plain" }] });
assert.equal(bundle.html.bytes, 12);
assert.equal(bundle.assets.length, 1);
assert.equal(bundle.assets[0].bytes, 5);
assert.equal(bundle.digest.length, 64);

const calls = [];
const fetcher = async (path, options = {}) => {
  calls.push({ path, options });
  if (path.endsWith("/prepare")) return new Response(JSON.stringify({ missing: [bundle.html.sha256, bundle.assets[0].sha256] }), { status: 200 });
  if (path.includes("/objects/")) return new Response("{}", { status: 200 });
  if (path.endsWith("/activate")) return new Response(JSON.stringify({ publication: { id: "pub-1" } }), { status: 200 });
  return new Response(JSON.stringify({ publication: null }), { status: 200 });
};
const published = await createPublicationPublisher({ slug: "paper", fetcher }).publish({ html: "<p>hello</p>", assets: [{ path: "public/a.txt", bytes }], sourceRevision: "a".repeat(64) });
assert.equal(published.id, "pub-1");
assert.equal(calls.filter((call) => call.path.includes("/objects/")).length, 2);
const prepare = calls.find((call) => call.path.endsWith("/prepare"));
const prepareBody = JSON.parse(prepare.options.body);
assert.equal(prepareBody.expected_publication_id, null);
assert.equal(prepareBody.manifest.source_sha256, "a".repeat(64));
assert.equal(prepareBody.manifest.bundle_sha256.length, 64);
const activate = calls.find((call) => call.path.endsWith("/activate"));
assert.deepEqual(Object.keys(JSON.parse(activate.options.body)).sort(), ["expected_publication_id", "manifest"]);

// A warm publication reuses every object the server says it already owns.
const warmCalls = [];
const warmFetcher = async (path, options = {}) => {
  warmCalls.push({ path, options });
  if (path.endsWith("/prepare")) return new Response(JSON.stringify({ missing: [bundle.html.sha256] }), { status: 200 });
  if (path.endsWith("/activate")) return new Response(JSON.stringify({ publication: { id: "pub-2" } }), { status: 200 });
  return new Response("{}", { status: 200 });
};
await createPublicationPublisher({ slug: "paper", fetcher: warmFetcher }).publish({ html: "<p>hello</p>", assets: [{ path: "public/a.txt", bytes }], sourceRevision: "b".repeat(64) });
assert.equal(warmCalls.filter((call) => call.path.includes("/objects/")).length, 1);

// Text objects use gzip on the wire when it saves space.  The server hashes
// the decoded object, so this checks the actual bytes sent rather than merely
// the request header.
if (typeof CompressionStream === "function" && typeof DecompressionStream === "function") {
  const gzipCalls = [];
  const longHtml = `<p>${"publication text ".repeat(800)}</p>`;
  const gzipFetcher = async (path, options = {}) => {
    gzipCalls.push({ path, options });
    if (path.endsWith("/prepare")) return new Response(JSON.stringify({ missing: [await sha256(longHtml)] }), { status: 200 });
    if (path.endsWith("/activate")) return new Response(JSON.stringify({ publication: { id: "pub-gzip" } }), { status: 200 });
    return new Response("{}", { status: 200 });
  };
  await createPublicationPublisher({ slug: "paper", fetcher: gzipFetcher }).publish({ html: longHtml });
  const uploaded = gzipCalls.find((call) => call.path.includes("/objects/"));
  assert.equal(uploaded.options.headers["content-encoding"], "gzip");
  const decoded = await new Response(new Blob([uploaded.options.body]).stream().pipeThrough(new DecompressionStream("gzip"))).text();
  assert.equal(decoded, longHtml, "the compressed upload decodes to the object whose hash was prepared");
}

let shown;
const reader = createPublicationReader({ slug: "paper", fetcher, onPublication: (value) => { shown = value; } });
await reader.refresh();
assert.equal(shown, null);
assert.ok(calls.every(({ path }) => !/\/source$|\/snapshot$|\/state$|\/history/.test(path)), "reader publication refresh never requests source state");
assert.equal(reader.announce({ id: "pub-2" }), true);
assert.equal(shown.pending_id, "pub-2");
assert.equal(shown.id, undefined, "announcement preserves the draft without replacing current publication metadata");
reader.dispose();
console.log("publication: digest, allowlist, missing-object upload and refresh behavior passed");

// Measure each transfer class with a large incompressible display image. The
// fixture models the server's per-project object store and current-only GC.
const imageBytes = new Uint8Array(1024 * 1024);
for (let start = 0; start < imageBytes.length; start += 65536) crypto.getRandomValues(imageBytes.subarray(start, start + 65536));
const stored = new Map();
const measurements = [];
let active = null;
let measurement;
const costFetcher = async (path, options = {}) => {
  if (path.endsWith("/prepare")) {
    measurement.metadataRequests += 1;
    measurement.metadataUploadBytes += encoderBytes(options.body);
    const { manifest, expected_publication_id } = JSON.parse(options.body);
    assert.equal(expected_publication_id, active?.id ?? null);
    return Response.json({ missing: [manifest.html, ...manifest.assets].filter((object) => !stored.has(object.sha256)).map((object) => object.sha256) });
  }
  if (path.includes("/objects/")) {
    const hash = path.split("/").at(-1);
    measurement.uploadBytes += options.body.byteLength;
    let decoded = options.body;
    if (options.headers["content-encoding"] === "gzip") decoded = new Uint8Array(await new Response(new Blob([decoded]).stream().pipeThrough(new DecompressionStream("gzip"))).arrayBuffer());
    assert.equal(await sha256(decoded), hash);
    stored.set(hash, decoded);
    if (options.headers["content-type"] === "image/png") measurement.imageUploadBytes += options.body.byteLength;
    return Response.json({});
  }
  assert.ok(path.endsWith("/activate"));
  measurement.metadataRequests += 1;
  measurement.metadataUploadBytes += encoderBytes(options.body);
  const { manifest } = JSON.parse(options.body);
  active = { id: `cost-${measurements.length}`, ...manifest };
  const reachable = new Set([manifest.html.sha256, ...manifest.assets.map((asset) => asset.sha256)]);
  for (const hash of stored.keys()) if (!reachable.has(hash)) stored.delete(hash);
  measurement.storedBytes = [...stored.values()].reduce((sum, value) => sum + value.byteLength, 0);
  return Response.json({ publication: active });
};
function encoderBytes(value) { return new TextEncoder().encode(value).byteLength; }
const costPublisher = createPublicationPublisher({ slug: "cost", fetcher: costFetcher });
const imageHash = await sha256(imageBytes);
const readerCache = new Map();
for (const [index, word] of ["original", "changed", "changed"].entries()) {
  measurement = { metadataRequests: 0, metadataUploadBytes: 0, uploadBytes: 0, imageUploadBytes: 0, storedBytes: 0, downloadBytes: 0, cacheReuses: 0 };
  measurements.push(measurement);
  await costPublisher.publish({ html: `<p>${word}</p><img loading="lazy" src="assets/${imageHash}.png">`, assets: [{ path: `assets/${imageHash}.png`, mime: "image/png", bytes: imageBytes }], sourceRevision: String(index + 1).repeat(64), expectedPublicationId: active?.id ?? null });
  assert.equal(active.source_sha256, String(index + 1).repeat(64));
  // An explicit reader refresh revalidates hashes; unchanged bodies cost zero.
  measurement.metadataRequests += 1;
  for (const object of [active.html, ...active.assets]) {
    if (readerCache.has(object.sha256)) measurement.cacheReuses += 1;
    else { measurement.downloadBytes += stored.get(object.sha256).byteLength; readerCache.set(object.sha256, true); }
  }
}
assert.equal(measurements[0].imageUploadBytes, imageBytes.byteLength);
assert.equal(measurements[1].imageUploadBytes, 0);
assert.ok(measurements[1].uploadBytes < 200);
assert.equal(measurements[1].cacheReuses, 1);
assert.equal(measurements[2].uploadBytes, 0);
assert.equal(measurements[2].downloadBytes, 0);
assert.equal(measurements[2].cacheReuses, 2);
assert.equal(measurements[1].storedBytes, measurements[2].storedBytes);
console.log("publication cost fixture (object bytes; HTTP overhead excluded):", JSON.stringify(measurements));
