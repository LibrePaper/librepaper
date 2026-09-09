import { rendererRequest } from "./renderer-client.js";

export function analyzeBibliography(request) {
  const url = globalThis.KOMODOC_MODULES?.bibliography;
  if (!url) return Promise.reject(new Error("Bibliography support is unavailable in this build."));
  return rendererRequest(new URL(url, globalThis.location.href).href, "bibliography", {
    main: request.main || "", format: request.format || "", source: request.source || "", texts: { ...request.texts },
  });
}

export function needsBibliography({ source = "" } = {}) {
  return /^bibliography\s*:/m.test(source) || /(^|[^\p{L}\p{N}_/:.@-])-?@[\p{L}\p{N}_]/u.test(source);
}
