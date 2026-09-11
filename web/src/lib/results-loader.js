import { prepareResultsArtifact } from "./results-artifact.js";

// Only the newest complete load may replace the displayed bundle. Failed
// downloads leave the last good resources alive; superseded loads release
// their newly prepared URLs without touching the currently displayed URLs.
export function createResultsLoader({ api, apply, prepare = prepareResultsArtifact }) {
  let epoch = 0;
  let flight = null;
  let loadedContext = "";
  let value = null;
  return {
    invalidate() { epoch += 1; flight = null; loadedContext = ""; },
    load(context, { force = false } = {}) {
      if (!force && flight?.context === context) return flight.promise;
      if (!force && loadedContext === context) return Promise.resolve(value);
      const serial = ++epoch;
      const promise = (async () => {
        const response = await api.selectedResults(context);
        const descriptor = await response.json().catch(() => null);
        if (serial !== epoch) return null;
        if (!response.ok && response.status !== 404) throw new Error(descriptor?.error || "Saved results could not be loaded.");
        if (response.status === 404) {
          const missing = { context, manifest:null, prepared:null, generation:Number(descriptor?.generation || 0) };
          apply(missing);
          value = missing;
          loadedContext = context;
          return missing;
        }
        const manifest = descriptor?.manifest;
        if (!manifest?.render_id || manifest.context?.id !== context) throw new Error("The saved Quarto bundle has an invalid context.");
        let artifactBytes = null;
        if (manifest.artifact) {
          const artifact = await api.resultArtifact(manifest.render_id);
          if (!artifact.ok) throw new Error("The saved results artifact could not be loaded.");
          artifactBytes = new Uint8Array(await artifact.arrayBuffer());
        }
        const prepared = await prepare(manifest, artifactBytes, async (asset) => {
          const response = await api.resultAsset(manifest.render_id, asset.path);
          if (!response.ok) throw new Error(`Saved result resource is missing: ${asset.path}`);
          return new Uint8Array(await response.arrayBuffer());
        });
        if (serial !== epoch) { prepared.dispose(); return null; }
        const loaded = { context, manifest, prepared, generation:Number(descriptor.selection?.generation || 0) };
        apply(loaded);
        value = loaded;
        loadedContext = context;
        return loaded;
      })().finally(() => { if (serial === epoch) flight = null; });
      flight = { context, promise };
      return promise;
    },
  };
}
