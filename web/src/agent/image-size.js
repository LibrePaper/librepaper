// The natural pixel size of an image, read from its first bytes. The preview
// gives each figure width and height attributes so the browser knows the
// aspect ratio before the image has decoded and the page does not jump.
// Returns { width, height }, or null for anything it cannot read. Never throws.

const u16be = (b, i) => (b[i] << 8) | b[i + 1];
const u16le = (b, i) => b[i] | (b[i + 1] << 8);
const u24le = (b, i) => b[i] | (b[i + 1] << 8) | (b[i + 2] << 16);
const u32be = (b, i) => ((b[i] << 24) | (b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3]) >>> 0;
const ascii = (b, i, text) => {
  if (i + text.length > b.length) return false;
  for (let k = 0; k < text.length; k++) if (b[i + k] !== text.charCodeAt(k)) return false;
  return true;
};
const size = (width, height) => (width > 0 && height > 0 ? { width, height } : null);

function png(b) {
  if (b.length < 24 || !ascii(b, 12, "IHDR")) return null;
  return size(u32be(b, 16), u32be(b, 20));
}

function gif(b) {
  if (b.length < 10) return null;
  return size(u16le(b, 6), u16le(b, 8));
}

function jpeg(b) {
  let i = 2;
  while (i + 3 < b.length) {
    if (b[i] !== 0xff) { i++; continue; }
    const marker = b[i + 1];
    if (marker === 0xff) { i++; continue; }
    // Markers without a length: padding, TEM, RSTn, SOI, EOI.
    if (marker === 0x00 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd9)) { i += 2; continue; }
    const length = u16be(b, i + 2);
    const isFrame = marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc;
    if (isFrame) {
      if (i + 8 >= b.length) return null;
      return size(u16be(b, i + 7), u16be(b, i + 5));
    }
    if (length < 2) return null;
    i += 2 + length;
  }
  return null;
}

function webp(b) {
  if (b.length < 30 || !ascii(b, 8, "WEBP")) return null;
  if (ascii(b, 12, "VP8 ")) {
    if (b[23] !== 0x9d || b[24] !== 0x01 || b[25] !== 0x2a) return null;
    return size(u16le(b, 26) & 0x3fff, u16le(b, 28) & 0x3fff);
  }
  if (ascii(b, 12, "VP8L")) {
    if (b[20] !== 0x2f) return null;
    const bits = (b[21] | (b[22] << 8) | (b[23] << 16) | (b[24] << 24)) >>> 0;
    return size((bits & 0x3fff) + 1, ((bits >>> 14) & 0x3fff) + 1);
  }
  if (ascii(b, 12, "VP8X")) return size(u24le(b, 24) + 1, u24le(b, 27) + 1);
  return null;
}

export function imageSize(bytes) {
  try {
    if (!bytes || bytes.length < 10) return null;
    if (bytes[0] === 0x89 && ascii(bytes, 1, "PNG")) return png(bytes);
    if (ascii(bytes, 0, "GIF8")) return gif(bytes);
    if (bytes[0] === 0xff && bytes[1] === 0xd8) return jpeg(bytes);
    if (ascii(bytes, 0, "RIFF")) return webp(bytes);
    return null;
  } catch {
    return null;
  }
}
