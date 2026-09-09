// Publication can commit an immutable bundle while losing the independent
// selection race. A retry must acknowledge those saved bytes without replacing
// the newer selection or asking the author to execute the analysis again.
function submittedFieldsMatch(saved, submitted) {
  if (submitted === null || typeof submitted !== "object") return saved === submitted;
  if (!saved || typeof saved !== "object") return false;
  if (Array.isArray(submitted)) return Array.isArray(saved) && saved.length === submitted.length && submitted.every((value, index) => submittedFieldsMatch(saved[index], value));
  return Object.entries(submitted).every(([key, value]) => Object.hasOwn(saved, key) && submittedFieldsMatch(saved[key], value));
}

export async function publishResultsBundle(api, payload, knownManifest = null) {
  const known = new Set([knownManifest?.artifact?.sha256, ...(knownManifest?.assets || []).map((asset) => asset.sha256)].filter(Boolean));
  const reduced = { ...payload, blobs: payload.blobs.filter((blob) => !known.has(blob.sha256)) };
  const publish = api.publishResults || api.quartoPublish;
  const bundle = api.resultBundle || api.quartoBundle;
  let response = await publish.call(api, reduced);
  // Another selection/GC pass can retire a previously known blob. Retry the
  // original bounded payload once, preserving the render ID and generation.
  if (response.status === 400 && reduced.blobs.length !== payload.blobs.length) response = await publish.call(api, payload);
  if (response.ok) return response.json();
  if (response.status === 409) {
    const saved = await bundle.call(api, payload.manifest.render_id);
    if (saved.ok) {
      const manifest = await saved.json();
      if (submittedFieldsMatch(manifest, payload.manifest)) return { manifest, selected:false, selection:null };
    }
  }
  const failure = await response.json().catch(() => null);
  throw new Error(failure?.error || failure?.message || `Sharing failed (${response.status}).`);
}

export const publishResultBundle = publishResultsBundle;
