// The document-scoped endpoints used by the preview controllers. Keeping the
// URL and credential rules here makes it harder for a new preview path to
// accidentally omit the shell or link-key headers.

export function createPreviewApi({ slug, key, shellHeaders, keyHeaders, request = fetch }) {
  const headers = { ...shellHeaders, ...keyHeaders(key) };

  const api = {
    frame() {
      return request(`/api/documents/${slug}/frame`, { headers });
    },
  };
  return api;
}
