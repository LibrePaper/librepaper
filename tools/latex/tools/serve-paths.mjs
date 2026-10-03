import { realpathSync, statSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";

const escaped = (root, path) => {
  const rel = relative(root, path);
  return rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel);
};

export function resolveLocalFile(root, encodedRelative) {
  let decoded;
  try { decoded = decodeURIComponent(encodedRelative); } catch { return null; }
  if (decoded.includes("\0")) return null;
  let realRoot;
  let realPath;
  try {
    realRoot = realpathSync(root);
    realPath = realpathSync(resolve(realRoot, decoded));
    if (escaped(realRoot, realPath) || !statSync(realPath).isFile()) return null;
  } catch {
    return null;
  }
  return realPath;
}

export function mirrorTarget(base, encodedSuffix) {
  let suffix;
  try { suffix = decodeURIComponent(encodedSuffix); } catch { return null; }
  if (!suffix || /[%?#:\0\\]/.test(suffix) || suffix.startsWith("/") ||
      suffix.split("/").some((part) => !part || part === "." || part === "..")) return null;
  let origin;
  try { origin = new URL(base); } catch { return null; }
  if (!["http:", "https:"].includes(origin.protocol) || origin.username || origin.password || origin.search || origin.hash) return null;
  if (!origin.pathname.endsWith("/")) origin.pathname += "/";
  const target = new URL(suffix, origin);
  if (target.origin !== origin.origin || !target.pathname.startsWith(origin.pathname) || target.search || target.hash) return null;
  return target;
}

export async function fetchMirror(target, options = {}) {
  const response = await fetch(target, { ...options, redirect: "manual" });
  return response.status >= 300 && response.status < 400 ? null : response;
}
