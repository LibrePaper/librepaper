// A zip file, built in the browser.
//
// Written here rather than taken from a package, because what is needed is the
// smallest part of the format: entries stored rather than deflated, no
// encryption, no zip64, no directory entries. That is a local header and a
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
    return { name: encoder.encode(path), bytes, crc: crc32(bytes) };
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
    out.u32(0);
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
