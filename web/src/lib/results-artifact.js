// Turn a complete, authenticated shared-results bundle into browser-local display resources.
// Relative URLs are resolved against the file that contains them, including
// nested CSS imports and fonts. No URL is resolved against the app's location.
import { validateResultsManifest } from "./engines/identity.js";

const ROOT = "https://librepaper-results.invalid/";

function dataUrl(bytes, mime) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return `data:${mime};base64,${btoa(binary)}`;
}

export function resultsArtifactReference(value, containingPath) {
  const raw = String(value).trim();
  if (!raw || raw.startsWith("#") || /^(?:data:|blob:|https?:|mailto:|\/\/)/i.test(raw)) return null;
  if (/[\u0000-\u001f\u007f\\]/.test(raw) || /^[A-Za-z][A-Za-z0-9+.-]*:/.test(raw)) return null;
  const url = new URL(raw, new URL(containingPath, ROOT));
  if (url.origin !== new URL(ROOT).origin) return null;
  let path;
  try { path = decodeURIComponent(url.pathname.slice(1)); } catch { return null; }
  if (!path || path.split("/").some((part) => !part || part === "." || part === "..")) return null;
  return { path, fragment:url.hash };
}

async function verified(bytes, descriptor) {
  const value = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  if (value.byteLength !== Number(descriptor.size)) throw new Error(`Saved result resource has the wrong size: ${descriptor.path || descriptor.entrypoint}`);
  const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", value))].map((part) => part.toString(16).padStart(2, "0")).join("");
  if (digest !== descriptor.sha256) throw new Error(`Saved result resource has the wrong digest: ${descriptor.path || descriptor.entrypoint}`);
  return value;
}

export async function prepareResultsArtifact(manifest, artifactBytes, readAsset) {
  if (!manifest) throw new Error("The saved results bundle is missing.");
  validateResultsManifest(manifest);
  const urls = [];
  const dispose = () => { for (const url of urls.splice(0)) URL.revokeObjectURL(url); };
  const blob = (bytes, mime) => { const url = URL.createObjectURL(new Blob([bytes], { type:mime })); urls.push(url); return url; };
  try {
    const artifact = manifest.artifact ? await verified(artifactBytes, manifest.artifact) : null;
    const descriptors = new Map((manifest.assets || []).map((asset) => [asset.path, asset]));
    const bytes = new Map();
    // Bound parallel downloads; a scientific paper can have hundreds of files.
    const inventory = [...descriptors.values()];
    let next = 0;
    await Promise.all(Array.from({ length:Math.min(6, inventory.length) }, async () => {
      while (next < inventory.length) {
        const asset = inventory[next++];
        const downloaded = await readAsset(asset);
        if (downloaded == null) throw new Error(`Saved result resource is missing: ${asset.path}`);
        bytes.set(asset.path, await verified(downloaded, asset));
      }
    }));
    const assets = {};
    const building = new Set();
    function reference(value, containingPath, required = true) {
      const resolved = resultsArtifactReference(value, containingPath);
      if (!resolved) return value;
      if (!descriptors.has(resolved.path)) {
        if (required) throw new Error(`Saved result resource is missing: ${resolved.path}`);
        return value;
      }
      return materialize(resolved.path) + resolved.fragment;
    }
    function css(source, containingPath) {
      // Quarto's generated stylesheets use standard url() and quoted @import.
      // Resolve imports before URL values so a rewritten blob URL stays intact.
      return source.replace(/(@import\s+)(['"])([^'"]+)\2/gi, (_, prefix, quote, value) => `${prefix}${quote}${reference(value, containingPath)}${quote}`)
        .replace(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)'"\s][^)]*))\s*\)/gi, (_, double, single, plain) => `url("${reference(double ?? single ?? plain.trim(), containingPath).replaceAll('"', '%22')}")`);
    }
    function materialize(path) {
      if (assets[path]) return assets[path];
      if (building.has(path)) throw new Error(`Cyclic stylesheet dependency cannot be previewed: ${path}`);
      building.add(path);
      const descriptor = descriptors.get(path);
      let content = bytes.get(path);
      if (descriptor.mime === "text/css") content = css(new TextDecoder().decode(content), path);
      // Image SVG is inert, but navigating to an app-origin SVG/HTML blob
      // could run its scripts with app authority. Keep SVG on an opaque data
      // URL and make other active-capable source dependencies plain text.
      const url = descriptor.mime === "image/svg+xml"
        ? dataUrl(content, descriptor.mime)
        : blob(content, /html|javascript/.test(descriptor.mime) ? "text/plain" : descriptor.mime);
      assets[path] = url;
      building.delete(path);
      return url;
    }
    for (const path of descriptors.keys()) materialize(path);
    if (!manifest.artifact) return { kind:null, assets, urls, dispose };
    const kind = manifest.artifact.kind;
    // Binary artifacts such as PDF remain available to the existing reader
    // as authenticated bytes. The download URL is kept separately so the
    // original artifact can still be downloaded with its declared MIME.
    if (kind !== "html") return { kind, bytes: artifact, assets, urls, dispose, downloadUrl:blob(artifact, manifest.artifact.mime) };
    const parsed = new DOMParser().parseFromString(new TextDecoder().decode(artifact), "text/html");
    // Full preview is static in this adapter version. The original bytes are
    // still available as a download; scripts/widgets are not transplanted into
    // either the app or the document agent's execution environment.
    for (const active of parsed.querySelectorAll("script, iframe, frame, frameset, object, embed, portal, form, meta[http-equiv], link[rel=import], animate, animateMotion, animateTransform, set, foreignObject")) active.remove();
    for (const element of parsed.querySelectorAll("*")) {
      for (const attribute of [...element.attributes]) {
        if (/^on/i.test(attribute.name) || attribute.name === "srcdoc") element.removeAttribute(attribute.name);
        if (/^(?:href|xlink:href|src|action|formaction)$/i.test(attribute.name) && /^[\s\u0000-\u0020]*(?:javascript|vbscript):/i.test(attribute.value.replace(/[\u0000-\u0020]/g, ""))) element.removeAttribute(attribute.name);
      }
    }
    for (const svg of parsed.querySelectorAll("svg")) {
      const image = parsed.createElement("img");
      image.src = dataUrl(new TextEncoder().encode(new XMLSerializer().serializeToString(svg)), "image/svg+xml");
      image.alt = svg.getAttribute("aria-label") || svg.querySelector("title")?.textContent || "";
      for (const name of ["width", "height", "class", "id"]) if (svg.hasAttribute(name)) image.setAttribute(name, svg.getAttribute(name));
      svg.replaceWith(image);
    }
    // A rendered document's base URL must not override the bundle's inventory.
    for (const base of parsed.querySelectorAll("base")) base.remove();
    const main = manifest.artifact.entrypoint;
    for (const element of parsed.querySelectorAll("[src], [srcset], [href], [poster], [style], style")) {
      for (const attribute of ["src", "poster"]) {
        if (element.hasAttribute(attribute)) element.setAttribute(attribute, reference(element.getAttribute(attribute), main));
      }
      if (element.hasAttribute("href")) element.setAttribute("href", reference(element.getAttribute("href"), main, element.localName === "link" && element.rel === "stylesheet"));
      if (element.hasAttribute("srcset") && !element.getAttribute("srcset").includes("data:")) {
        element.setAttribute("srcset", element.getAttribute("srcset").split(",").map((candidate) => {
          const match = /^(\S+)(.*)$/.exec(candidate.trim());
          return match ? reference(match[1], main) + match[2] : "";
        }).join(", "));
      }
      if (element.hasAttribute("style")) element.setAttribute("style", css(element.getAttribute("style"), main));
      if (element.localName === "style") element.textContent = css(element.textContent, main);
    }
    return { kind:"html", html:`<!doctype html>\n${parsed.documentElement.outerHTML}`, assets, urls, dispose,
      // Plain-text MIME prevents an app-origin HTML execution context even
      // if this download URL is opened directly instead of saved to disk.
      downloadUrl:blob(artifact, "text/plain;charset=utf-8"), staticPreview:true };
  } catch (error) {
    dispose();
    throw error;
  }
}

// Concise generic aliases used by new readers; the longer names make call
// sites that handle several result types self-documenting.
export const prepareArtifact = prepareResultsArtifact;
export const artifactReference = resultsArtifactReference;
