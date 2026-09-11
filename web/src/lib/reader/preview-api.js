// The document-scoped endpoints used by the preview controllers. Keeping the
// URL and credential rules here makes it harder for a new preview path to
// accidentally omit the shell or link-key headers.

export function createPreviewApi({ slug, key, shellHeaders, keyHeaders, request = fetch }) {
  const headers = { ...shellHeaders, ...keyHeaders(key) };
  const latestPath = `/api/documents/${slug}/renderings/latest`;

  const api = {
    frame() {
      return request(`/api/documents/${slug}/frame`, { headers });
    },
    latest() {
      return request(latestPath, { headers });
    },
    rendering(sha) {
      return request(`/api/documents/${slug}/renderings/${sha}`, { headers });
    },
    // Quarto bundles are immutable records separate from the legacy PDF
    // rendering row. Deployments may return an inline artifact descriptor or
    // a URL; callers keep both forms behind this document-scoped API.
    // Shared-results names use the existing Quarto routes until another
    // execution engine is introduced.
    selectedResults(context = "html") {
      return request(`/api/documents/${slug}/quarto/bundles/selected/${encodeURIComponent(context)}`, { headers });
    },
    resultBundle(renderId) {
      return request(`/api/documents/${slug}/quarto/bundles/${encodeURIComponent(renderId)}`, { headers });
    },
    publishResults(bundle) {
      return request(`/api/documents/${slug}/quarto/bundles`, {
        method: "POST",
        headers: { ...headers, "content-type": "application/json" },
        body: JSON.stringify(bundle),
      });
    },
    resultArtifact(renderId, name = "artifact.html") {
      return request(`/api/documents/${slug}/quarto/bundles/${encodeURIComponent(renderId)}/artifact`, { headers });
    },
    resultAsset(renderId, path) {
      return request(`/api/documents/${slug}/quarto/bundles/${encodeURIComponent(renderId)}/asset?path=${encodeURIComponent(String(path))}`, { headers });
    },
    putRendering(name, suffix, body, provenance = null) {
      const putHeaders = provenance
        ? { ...headers, "x-librepaper-provenance": JSON.stringify(provenance) }
        : headers;
      return request(`/api/documents/${slug}/renderings/${name}${suffix}`, {
        method: "PUT",
        headers: putHeaders,
        body,
      });
    },
  };
  return api;
}
