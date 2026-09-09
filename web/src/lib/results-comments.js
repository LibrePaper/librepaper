import { sha256 } from "./results-hash.js";
import { prepareResultsArtifact } from "./results-artifact.js";

export function savedResultItems(manifest) {
  return (manifest?.cells || []).flatMap((cell) => (cell.outputs || []).map((output) => {
    const asset = (manifest.assets || []).find((item) => item.path === output.asset);
    return { cell, output, digest:output.content_sha256 || asset?.sha256 || "" };
  }));
}

export function savedResultAnchor(manifest, item, dimensions = {}) {
  if (!manifest?.render_id || !/^[a-f0-9]{64}$/.test(item.digest)) throw new Error("This result has no verified content identity.");
  return { render_id:manifest.render_id, cell_id:item.cell.id, output_ordinal:item.output.ordinal,
    content_sha256:item.digest, coordinate_system:"percent", width:dimensions.width || 0, height:dimensions.height || 0 };
}

// Inspect the referenced render, never the current cell's replacement output.
export async function inspectSavedResult(api, anchor) {
  const bundle = api.resultBundle || api.quartoBundle;
  const response = await bundle.call(api, anchor.render_id);
  if (!response.ok) throw new Error("The original saved result is unavailable.");
  const body = await response.json();
  const manifest = body.manifest || body;
  if (manifest.render_id !== anchor.render_id) throw new Error("The saved result has a different render identity.");
  const item = savedResultItems(manifest).find((item) => item.cell.id === anchor.cell_id && item.output.ordinal === anchor.output_ordinal);
  if (!item || item.digest !== anchor.content_sha256) throw new Error("The saved result does not match this discussion.");
  if (item.output.text != null && await sha256(item.output.text) !== item.digest) throw new Error("The saved result text failed its integrity check.");
  const assets = item.output.asset ? manifest.assets.filter((asset) => asset.path === item.output.asset) : [];
  if (item.output.asset && assets.length !== 1) throw new Error("The saved result image is unavailable.");
  if (item.output.asset && assets[0].sha256 !== item.digest) throw new Error("The saved result image failed its integrity check.");
  const prepared = await prepareResultsArtifact({ ...manifest, artifact:null, assets }, null, async (asset) => {
    const fetchAsset = api.resultAsset || api.quartoAsset;
    const resource = await fetchAsset.call(api, anchor.render_id, asset.path);
    if (!resource.ok) throw new Error("The original result image is unavailable.");
    return new Uint8Array(await resource.arrayBuffer());
  });
  return { manifest, items:[item], assets:prepared.assets, dispose:prepared.dispose };
}

export const resultItems = savedResultItems;
export const resultAnchor = savedResultAnchor;
export const inspectResult = inspectSavedResult;
