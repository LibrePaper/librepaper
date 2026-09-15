import assert from "node:assert/strict";
import { deflateRawSync } from "node:zlib";
import { unzip, zip } from "../../src/lib/zip.js";

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let n = 0; n < 8; n++) crc = crc & 1 ? 0xedb88320 ^ (crc >>> 1) : crc >>> 1;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

{
  const archive = zip({ "paper/main.md": "# Paper", "paper/fig.png": new Uint8Array([1, 2, 3]) });
  assert.deepEqual(await unzip(archive), [
    { path: "paper/main.md", bytes: new TextEncoder().encode("# Paper") },
    { path: "paper/fig.png", bytes: new Uint8Array([1, 2, 3]) },
  ]);
}

// A tiny deflated entry proves imports from normal desktop ZIP tools work,
// while the writer above intentionally uses stored entries.
{
  const body = new TextEncoder().encode("hello");
  const compressed = new Uint8Array(deflateRawSync(body));
  const bytes = new Uint8Array(30 + 1 + compressed.length + 22 + 1 + 46);
  const view = new DataView(bytes.buffer);
  const put = (at, value) => view.setUint32(at, value, true);
  put(0, 0x04034b50); view.setUint16(4, 20, true); view.setUint16(8, 8, true);
  view.setUint32(14, crc32(body), true);
  view.setUint32(18, compressed.length, true); view.setUint32(22, body.length, true);
  view.setUint16(26, 1, true); bytes[30] = 97; bytes.set(compressed, 31);
  const central = 31 + compressed.length;
  put(central, 0x02014b50); view.setUint16(central + 10, 8, true);
  view.setUint32(central + 16, crc32(body), true); view.setUint32(central + 20, compressed.length, true); view.setUint32(central + 24, body.length, true);
  view.setUint16(central + 28, 1, true); view.setUint32(central + 42, 0, true); bytes[central + 46] = 97;
  const end = central + 47;
  put(end, 0x06054b50); view.setUint16(end + 8, 1, true); view.setUint16(end + 10, 1, true);
  view.setUint32(end + 12, 47, true); view.setUint32(end + 16, central, true);
  assert.deepEqual(await unzip(new Blob([bytes])), [{ path: "a", bytes: body }]);
  // A forged smaller size must stop streaming inflation, even with a valid
  // compressed stream. Trusting just the directory would permit a ZIP bomb.
  view.setUint32(22, 1, true); view.setUint32(central + 24, 1, true);
  await assert.rejects(unzip(new Blob([bytes])), /size limit/);
}

await assert.rejects(unzip(new Blob([new Uint8Array([1, 2, 3])])), /not a readable ZIP/);
await assert.rejects(unzip(zip({ "a.txt": "12345" }), { maxBytes: 4 }), /too large/);
await assert.rejects(unzip(zip({ "a.txt": "a", "b.txt": "b" }), { maxFiles: 1 }), /more than 1 files/);
await assert.rejects(unzip(zip({ "a.txt": "a", "A.txt": "b" })), /share one name/);
for (const path of ["../main.md", "/main.md", "C:/main.md", "paper/../../main.md", "paper\\main.md"]) {
  await assert.rejects(unzip(zip({ [path]: "text" })), /invalid path/);
}
// Reject an oversized compressed input before ever reading it into memory.
await assert.rejects(unzip({ size: 100000, arrayBuffer() { throw new Error("must not read"); } }, { maxBytes: 1, maxFiles: 1 }), /too large/);
// Corruption and unsupported features must fail before a project is uploaded.
async function altered(change) {
  const bytes = new Uint8Array(await zip({ "main.md": "# Paper" }).arrayBuffer());
  const view = new DataView(bytes.buffer);
  const end = bytes.length - 22;
  const central = view.getUint32(end + 16, true);
  change(bytes, view, central, end);
  return new Blob([bytes]);
}
await assert.rejects(unzip(await altered((bytes) => { bytes[37] ^= 1; })), /corrupt/);
await assert.rejects(unzip(await altered((bytes, view, central) => { view.setUint32(central + 38, 0xa1ff0000, true); })), /symbolic link/);
await assert.rejects(unzip(await altered((bytes, view, central) => { view.setUint16(central + 8, 1, true); })), /encrypted/);
await assert.rejects(unzip(await altered((bytes, view, central) => { view.setUint32(central + 24, 0xffffffff, true); })), /ZIP64/);
await assert.rejects(unzip(await altered((bytes, view, central, end) => { view.setUint16(end + 4, 1, true); })), /Split ZIP/);
await assert.rejects(unzip(await altered((bytes, view, central) => { view.setUint16(central + 30, 65535, true); })), /corrupt/);
await assert.rejects(unzip(await altered((bytes) => { bytes[30] = 120; })), /corrupt/);
assert.deepEqual(await unzip(zip({ "paper/": new Uint8Array(), "paper/main.md": "# Paper" }), { maxFiles: 1 }), [
  { path: "paper/main.md", bytes: new TextEncoder().encode("# Paper") },
]);
console.log("zip: stored/deflated round trips, malformed archives, unsafe paths, and actual byte limits passed");
