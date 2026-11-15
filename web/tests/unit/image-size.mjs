// The natural size the preview frame reads from a figure's bytes.
import assert from "node:assert/strict";
import { imageSize } from "../../src/agent/image-size.js";

const text = (s) => [...s].map((c) => c.charCodeAt(0));
const be32 = (n) => [(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255];
const be16 = (n) => [(n >> 8) & 255, n & 255];
const le16 = (n) => [n & 255, (n >> 8) & 255];
const le24 = (n) => [n & 255, (n >> 8) & 255, (n >> 16) & 255];

const png = (w, h) => Uint8Array.from([
  0x89, ...text("PNG"), 0x0d, 0x0a, 0x1a, 0x0a, ...be32(13), ...text("IHDR"), ...be32(w), ...be32(h), 8, 6, 0, 0, 0,
]);
assert.deepEqual(imageSize(png(640, 480)), { width: 640, height: 480 });

const gif = (w, h) => Uint8Array.from([...text("GIF89a"), ...le16(w), ...le16(h), 0, 0, 0]);
assert.deepEqual(imageSize(gif(33, 17)), { width: 33, height: 17 });

// A JPEG with an APP0 segment and a DHT before the frame header.
const jpeg = (w, h, sof = 0xc2) => Uint8Array.from([
  0xff, 0xd8,
  0xff, 0xe0, ...be16(6), 1, 2, 3, 4,
  0xff, 0xc4, ...be16(4), 0, 0,
  0xff, sof, ...be16(11), 8, ...be16(h), ...be16(w), 1, 1, 0x11, 0,
]);
assert.deepEqual(imageSize(jpeg(300, 200)), { width: 300, height: 200 });
assert.deepEqual(imageSize(jpeg(300, 200, 0xc0)), { width: 300, height: 200 });

const riff = (chunk, body) => Uint8Array.from([...text("RIFF"), 0, 0, 0, 0, ...text("WEBP"), ...text(chunk), 0, 0, 0, 0, ...body]);
assert.deepEqual(
  imageSize(riff("VP8 ", [0, 0, 0, 0x9d, 0x01, 0x2a, ...le16(120), ...le16(80), 0, 0])),
  { width: 120, height: 80 },
);
{
  const bits = (99 - 1) | ((49 - 1) << 14);
  assert.deepEqual(
    imageSize(riff("VP8L", [0x2f, bits & 255, (bits >> 8) & 255, (bits >> 16) & 255, (bits >>> 24) & 255, 0])),
    { width: 99, height: 49 },
  );
}
assert.deepEqual(
  imageSize(riff("VP8X", [0, 0, 0, 0, ...le24(1000 - 1), ...le24(500 - 1)])),
  { width: 1000, height: 500 },
);

// Garbage, empty and truncated input are not an error.
assert.equal(imageSize(null), null);
assert.equal(imageSize(new Uint8Array()), null);
assert.equal(imageSize(Uint8Array.of(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)), null);
assert.equal(imageSize(png(10, 10).slice(0, 20)), null);
assert.equal(imageSize(gif(1, 1).slice(0, 8)), null);
assert.equal(imageSize(jpeg(10, 10).slice(0, 20)), null);
assert.equal(imageSize(Uint8Array.of(0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0, 0, 0, 0)), null);
assert.equal(imageSize(riff("VP8X", [0, 0, 0, 0, 1]).slice(0, 26)), null);
assert.equal(imageSize(png(0, 5)), null);
