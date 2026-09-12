// A zip file, built in the browser.
//
// Written here rather than taken from a package, because what is needed is the
// smallest part of the format: entries stored rather than deflated, no
// encryption, no zip64. That is a local header and a
// central directory record per file and a fixed tail, which is the ninety
// lines below -- against a dependency whose compression this does not use and
// whose other ninety per cent would ship to every reader of every document.
//
// A paper's texts would compress well and its figures would not: a PNG and a
// PDF are already compressed, and they are most of the bytes. Storing rather
// than deflating costs a fraction on the texts and nothing on the rest, and
// buys a file every unzip program on earth opens.

/// CRC-32, which a zip entry carries for each file. The table is built once,
/// on first use, because building it costs a millisecond and a document with
/// no download never pays it.
let table = null;
function crcTable() {
  if (table) return table;
  table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
}

function crc32(bytes) {
  const of = crcTable();
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = of[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/// The date and time a zip entry carries, in the DOS shape the format has
/// used since 1989: seconds in units of two, and a year counted from 1980.
function dosTime(when) {
  const time =
    (when.getHours() << 11) | (when.getMinutes() << 5) | Math.floor(when.getSeconds() / 2);
  const date =
    ((when.getFullYear() - 1980) << 9) | ((when.getMonth() + 1) << 5) | when.getDate();
  return { time, date };
}

class Writer {
  constructor(size) {
    this.bytes = new Uint8Array(size);
    this.view = new DataView(this.bytes.buffer);
    this.at = 0;
  }
  u16(value) {
    this.view.setUint16(this.at, value, true);
    this.at += 2;
  }
  u32(value) {
    this.view.setUint32(this.at, value >>> 0, true);
    this.at += 4;
  }
  raw(bytes) {
    this.bytes.set(bytes, this.at);
    this.at += bytes.length;
  }
}

/// A zip of `files`, which is a map of path to bytes or string. Answers a
/// `Blob`, which is what a download needs.
///
/// Paths are written as they are: a zip entry's name is a `/`-separated
/// relative path, which is exactly what a document's paths already are.
export function zip(files) {
  const encoder = new TextEncoder();
  const entries = Object.entries(files).map(([path, body]) => {
    const bytes = typeof body === "string" ? encoder.encode(body) : body;
    return { name: encoder.encode(path), directory: path.endsWith("/"), bytes, crc: crc32(bytes) };
  });
  const { time, date } = dosTime(new Date());

  const local = entries.reduce((sum, one) => sum + 30 + one.name.length + one.bytes.length, 0);
  const central = entries.reduce((sum, one) => sum + 46 + one.name.length, 0);
  const out = new Writer(local + central + 22);

  const offsets = [];
  for (const one of entries) {
    offsets.push(out.at);
    out.u32(0x04034b50); // local file header
    out.u16(20); // the version that can read it: 2.0, which is stored entries
    out.u16(1 << 11); // filenames are UTF-8
    out.u16(0); // stored, not deflated
    out.u16(time);
    out.u16(date);
    out.u32(one.crc);
    out.u32(one.bytes.length);
    out.u32(one.bytes.length);
    out.u16(one.name.length);
    out.u16(0);
    out.raw(one.name);
    out.raw(one.bytes);
  }

  const directoryAt = out.at;
  entries.forEach((one, index) => {
    out.u32(0x02014b50); // central directory record
    out.u16(20); // made by
    out.u16(20); // needed to extract
    out.u16(1 << 11); // filenames are UTF-8
    out.u16(0);
    out.u16(time);
    out.u16(date);
    out.u32(one.crc);
    out.u32(one.bytes.length);
    out.u32(one.bytes.length);
    out.u16(one.name.length);
    out.u16(0);
    out.u16(0);
    out.u16(0);
    out.u16(0);
    out.u32(one.directory ? 0x10 : 0); // DOS directory attribute
    out.u32(offsets[index]);
    out.raw(one.name);
  });

  // Measured before the end record is written, not during it: `out.at` moves
  // as the record is laid down, and reading it halfway through counts twelve
  // of the end record's own bytes as directory. Every unzip then reports the
  // file as twelve bytes short.
  const directorySize = out.at - directoryAt;
  out.u32(0x06054b50); // end of central directory
  out.u16(0);
  out.u16(0);
  out.u16(entries.length);
  out.u16(entries.length);
  out.u32(directorySize);
  out.u32(directoryAt);
  out.u16(0);

  return new Blob([out.bytes], { type: "application/zip" });
}

// Read stored and deflated ZIPs without extracting anything onto a filesystem.
// Both the compressed input and actual decompressed bytes are bounded: central
// directory sizes are untrusted and cannot authorize an unbounded allocation.
export async function unzip(blob, { maxBytes = 64 * 1024 * 1024, maxFiles = 2000 } = {}) {
  const corrupt = () => new Error("The ZIP archive is corrupt.");
  const tooLarge = () => new Error("The archive is too large.");
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 0 || !Number.isSafeInteger(maxFiles) || maxFiles < 1) {
    throw new Error("Invalid archive limits.");
  }
  // Allow ZIP headers and a comment in addition to the document's byte budget.
  if (blob.size > maxBytes + maxFiles * 1024 + 65557) throw tooLarge();
  const bytes = new Uint8Array(await blob.arrayBuffer());
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let eocd = -1;
  for (let at = bytes.length - 22; at >= Math.max(0, bytes.length - 22 - 0xffff); at--) {
    if (view.getUint32(at, true) === 0x06054b50 && at + 22 + view.getUint16(at + 20, true) === bytes.length) {
      eocd = at;
      break;
    }
  }
  if (eocd < 0) throw new Error("This is not a readable ZIP archive.");
  const count = view.getUint16(eocd + 10, true);
  const centralSize = view.getUint32(eocd + 12, true);
  const centralAt = view.getUint32(eocd + 16, true);
  if (count === 0xffff || centralAt === 0xffffffff || centralSize === 0xffffffff) {
    throw new Error("ZIP64 archives are not supported.");
  }
  if (view.getUint16(eocd + 4, true) || view.getUint16(eocd + 6, true) || view.getUint16(eocd + 8, true) !== count) {
    throw new Error("Split ZIP archives are not supported.");
  }
  if (centralAt + centralSize !== eocd) throw corrupt();
  // Directory records do not consume the document's file count, but still
  // have a finite bound independent of how many empty directories were sent.
  if (count > Math.min(65534, maxFiles * 9)) throw new Error("The archive contains too many entries.");
  const entries = [];
  const names = new Set();
  let at = centralAt;
  let total = 0;
  for (let n = 0; n < count; n++) {
    if (at + 46 > eocd || view.getUint32(at, true) !== 0x02014b50) throw corrupt();
    const flags = view.getUint16(at + 8, true);
    const method = view.getUint16(at + 10, true);
    const compressed = view.getUint32(at + 20, true);
    const size = view.getUint32(at + 24, true);
    const expectedCrc = view.getUint32(at + 16, true);
    const nameSize = view.getUint16(at + 28, true);
    const extraSize = view.getUint16(at + 30, true);
    const commentSize = view.getUint16(at + 32, true);
    const external = view.getUint32(at + 38, true);
    const localAt = view.getUint32(at + 42, true);
    if (view.getUint16(at + 34, true)) throw new Error("Split ZIP archives are not supported.");
    if ([compressed, size, localAt].includes(0xffffffff)) throw new Error("ZIP64 archives are not supported.");
    if (at + 46 + nameSize + extraSize + commentSize > eocd) throw corrupt();
    const encodedName = bytes.subarray(at + 46, at + 46 + nameSize);
    let name;
    try { name = decoder.decode(encodedName); }
    catch { throw new Error("ZIP filenames must use UTF-8. Re-export the archive with UTF-8 names."); }
    at += 46 + nameSize + extraSize + commentSize;
    if (!name || name.startsWith("/") || name.includes("\\") || /^[a-z]:/i.test(name) || /[\u0000-\u001f\u007f]/.test(name)) throw new Error("The archive contains an invalid path.");
    const path = name.endsWith("/") ? name.slice(0, -1) : name;
    if (path.split("/").some(part => !part || part.trim() === "." || part.trim() === "..")) throw new Error("The archive contains an invalid path.");
    const key = path.trim().normalize("NFC").toLowerCase();
    if (names.has(key)) throw new Error(name + ": two archive entries cannot share one name.");
    names.add(key);
    if (flags & 0x41) throw new Error("The archive contains an encrypted file: " + name);
    const fileType = (external >>> 16) & 0xf000;
    if (fileType && fileType !== 0x8000 && fileType !== 0x4000) throw new Error("The archive contains a symbolic link or special file: " + name);
    if (method !== 0 && method !== 8) throw new Error("This archive uses unsupported compression for " + name + ".");
    if (localAt + 30 > centralAt || view.getUint32(localAt, true) !== 0x04034b50) throw corrupt();
    const localNameSize = view.getUint16(localAt + 26, true);
    const localExtraSize = view.getUint16(localAt + 28, true);
    const dataAt = localAt + 30 + localNameSize + localExtraSize;
    if (dataAt + compressed > centralAt || localNameSize !== nameSize || view.getUint16(localAt + 8, true) !== method || view.getUint16(localAt + 6, true) !== flags) throw corrupt();
    if (!encodedName.every((byte, i) => byte === bytes[localAt + 30 + i])) throw corrupt();
    if (!(flags & 8) && (view.getUint32(localAt + 14, true) !== expectedCrc || view.getUint32(localAt + 18, true) !== compressed || view.getUint32(localAt + 22, true) !== size)) throw corrupt();
    if (name.endsWith("/") || fileType === 0x4000) {
      if (size !== 0) throw corrupt();
      continue;
    }
    if (entries.length >= maxFiles) throw new Error("The archive contains more than " + maxFiles + " files.");
    if (size > maxBytes - total) throw tooLarge();
    total += size;
    entries.push({ path: name, dataAt, compressed, size, method, expectedCrc });
  }
  if (at !== eocd) throw corrupt();
  const result = [];
  for (const entry of entries) {
    let body = bytes.subarray(entry.dataAt, entry.dataAt + entry.compressed);
    if (entry.method === 8) {
      if (typeof DecompressionStream === "undefined") throw new Error("This browser cannot unpack compressed ZIP files. Update your browser or publish the directory with the CLI.");
      let inflated = 0;
      const limited = new TransformStream({
        transform(chunk, controller) {
          inflated += chunk.byteLength;
          if (inflated > entry.size) throw tooLarge();
          controller.enqueue(chunk);
        },
      });
      try {
        body = new Uint8Array(await new Response(new Blob([body]).stream().pipeThrough(new DecompressionStream("deflate-raw")).pipeThrough(limited)).arrayBuffer());
      } catch {
        throw new Error("Could not decompress " + entry.path + ": unsupported compression, corrupt data, or exceeded size limit.");
      }
    }
    if (body.length !== entry.size || crc32(body) !== entry.expectedCrc) throw new Error("The ZIP archive is corrupt near " + entry.path + ".");
    result.push({ path: entry.path, bytes: body });
  }
  return result;
}
