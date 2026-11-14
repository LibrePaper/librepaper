// The bundled fonts a document is drawn in, as bytes for the document frame.
//
// The frame is served from the documents origin, so it cannot fetch font files
// from the app: they are cross-origin. The app fetches the same woff2 files
// that styles/fonts.css declares for the app itself, and hands the bytes over.
// The agent in the frame registers them with the FontFace API.

import interLatin from "@fontsource-variable/inter/files/inter-latin-wght-normal.woff2?url";
import interLatinExt from "@fontsource-variable/inter/files/inter-latin-ext-wght-normal.woff2?url";
import serifLatin from "@fontsource-variable/source-serif-4/files/source-serif-4-latin-wght-normal.woff2?url";
import serifLatinExt from "@fontsource-variable/source-serif-4/files/source-serif-4-latin-ext-wght-normal.woff2?url";
import serifItalicLatin from "@fontsource-variable/source-serif-4/files/source-serif-4-latin-wght-italic.woff2?url";
import serifItalicLatinExt from "@fontsource-variable/source-serif-4/files/source-serif-4-latin-ext-wght-italic.woff2?url";

/**
 * @typedef {{ family: string, descriptors: { style: string, weight: string, unicodeRange: string }, data: ArrayBuffer }} DocumentFace
 */

// The descriptors are copied from the @font-face blocks in the packages'
// wght.css and wght-italic.css, the blocks whose src names the same file.
const FACES = [
  {
    family: "Inter",
    url: interLatin,
    descriptors: {
      style: "normal",
      weight: "100 900",
      unicodeRange: "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD",
    },
  },
  {
    family: "Inter",
    url: interLatinExt,
    descriptors: {
      style: "normal",
      weight: "100 900",
      unicodeRange: "U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF",
    },
  },
  {
    family: "Source Serif 4",
    url: serifLatin,
    descriptors: {
      style: "normal",
      weight: "200 900",
      unicodeRange: "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD",
    },
  },
  {
    family: "Source Serif 4",
    url: serifLatinExt,
    descriptors: {
      style: "normal",
      weight: "200 900",
      unicodeRange: "U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF",
    },
  },
  {
    family: "Source Serif 4",
    url: serifItalicLatin,
    descriptors: {
      style: "italic",
      weight: "200 900",
      unicodeRange: "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD",
    },
  },
  {
    family: "Source Serif 4",
    url: serifItalicLatinExt,
    descriptors: {
      style: "italic",
      weight: "200 900",
      unicodeRange: "U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF",
    },
  },
];

/** @type {Promise<DocumentFace[]> | null} */
let loaded = null;

/// The bundled faces with their bytes, fetched once per page. Never rejects: a
/// face that fails to load is left out, and when none load the result is empty
/// and the memo is cleared, so a later call tries again.
/// @returns {Promise<DocumentFace[]>}
export function documentFonts() {
  if (!loaded) {
    loaded = Promise.all(FACES.map(loadFace)).then((results) => {
      const faces = results.filter((face) => face !== null);
      if (faces.length === 0) loaded = null;
      return faces;
    });
  }
  return loaded;
}

/// One face's bytes, or null when the fetch fails or the response is not ok.
/// @param {(typeof FACES)[number]} face
/// @returns {Promise<DocumentFace | null>}
async function loadFace(face) {
  try {
    const response = await fetch(face.url);
    if (!response.ok) return null;
    return { family: face.family, descriptors: face.descriptors, data: await response.arrayBuffer() };
  } catch {
    return null;
  }
}
