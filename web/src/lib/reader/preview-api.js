// The document-scoped endpoints used by the preview controllers. Keeping the
// URL and credential rules here makes it harder for a new preview path to
// accidentally omit the shell or link-key headers.

export function createPreviewApi({ slug, key, shellHeaders, keyHeaders, request = fetch }) {
  const headers = { ...shellHeaders, ...keyHeaders(key) };
  const latestPath = `/api/documents/${slug}/renderings/latest`;

  return {
    frame() {
      return request(`/api/documents/${slug}/frame`, { headers });
    },
    latest() {
      return request(latestPath, { headers });
    },
    rendering(sha) {
      return request(`/api/documents/${slug}/renderings/${sha}`, { headers });
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
}
