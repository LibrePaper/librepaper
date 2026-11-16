// Behavioral checks for the iOS-only viewport cap that stops Safari focus zoom.
import assert from "node:assert/strict";
import { test } from "node:test";
import { preventFocusZoom } from "../../src/lib/ios-viewport.js";

const VIEWPORT = "width=device-width,initial-scale=1";

function fakeDoc(content) {
  const meta = content === null ? null : { content };
  return { querySelector: (selector) => (selector === 'meta[name="viewport"]' ? meta : null), meta };
}

const iPhone = { platform: "iPhone", maxTouchPoints: 5 };
const iPad = { platform: "MacIntel", maxTouchPoints: 5 };
const desktopMac = { platform: "MacIntel", maxTouchPoints: 0 };
const androidLinux = { platform: "Linux armv8l", maxTouchPoints: 5 };

test("appends maximum-scale on iPhone", () => {
  const doc = fakeDoc(VIEWPORT);
  preventFocusZoom(doc, iPhone);
  assert.equal(doc.meta.content, `${VIEWPORT},maximum-scale=1`);
});

test("appends maximum-scale on iPad reporting as MacIntel with touch points", () => {
  const doc = fakeDoc(VIEWPORT);
  preventFocusZoom(doc, iPad);
  assert.equal(doc.meta.content, `${VIEWPORT},maximum-scale=1`);
});

test("leaves a desktop Mac unchanged", () => {
  const doc = fakeDoc(VIEWPORT);
  preventFocusZoom(doc, desktopMac);
  assert.equal(doc.meta.content, VIEWPORT);
});

test("leaves Android unchanged", () => {
  const doc = fakeDoc(VIEWPORT);
  preventFocusZoom(doc, androidLinux);
  assert.equal(doc.meta.content, VIEWPORT);
});

test("does not duplicate an existing maximum-scale", () => {
  const capped = `${VIEWPORT},maximum-scale=1`;
  const doc = fakeDoc(capped);
  preventFocusZoom(doc, iPhone);
  assert.equal(doc.meta.content, capped);
});

test("does not throw when the viewport meta is missing", () => {
  const doc = fakeDoc(null);
  assert.doesNotThrow(() => preventFocusZoom(doc, iPhone));
});
