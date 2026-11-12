import assert from "node:assert/strict";
import test from "node:test";
import {
  buildMetadata,
  cleanTypes,
  decodeClipboardHex,
  isValidName,
  parseArgs,
} from "./paste-capture.mjs";

test("parseArgs reads the name and the optional metadata flags", () => {
  assert.deepEqual(
    parseArgs(["google-docs-chrome", "--app", "Google Docs", "--browser", "Chrome 142", "--note", "Select all first"]),
    {
      name: "google-docs-chrome",
      app: "Google Docs",
      browser: "Chrome 142",
      note: "Select all first",
      force: false,
    },
  );
});

test("parseArgs accepts --force in any position", () => {
  assert.equal(parseArgs(["libreoffice-writer", "--force"]).force, true);
  assert.equal(parseArgs(["--force", "libreoffice-writer"]).force, true);
  assert.equal(parseArgs(["libreoffice-writer"]).force, false);
});

test("parseArgs rejects a missing name", () => {
  assert.throws(() => parseArgs([]), /missing <name>/);
  assert.throws(() => parseArgs(["--force"]), /missing <name>/);
});

test("parseArgs rejects an option without a value and unknown options", () => {
  assert.throws(() => parseArgs(["web-page-chrome", "--app"]), /--app needs a value/);
  assert.throws(() => parseArgs(["web-page-chrome", "--app", "--force"]), /--app needs a value/);
  assert.throws(() => parseArgs(["web-page-chrome", "--colour", "x"]), /unknown option --colour/);
  assert.throws(() => parseArgs(["a", "b"]), /unexpected argument b/);
});

test("isValidName accepts the documented shape and rejects everything else", () => {
  for (const name of ["google-docs-chrome", "libreoffice-writer", "word-online-firefox", "web-page-chrome", "a1"]) {
    assert.equal(isValidName(name), true, name);
  }
  for (const name of ["", "Google-Docs", "-leading", "trailing-", "double--hyphen", "has space", "under_score", "dot.name", "../escape"]) {
    assert.equal(isValidName(name), false, JSON.stringify(name));
  }
  assert.equal(isValidName(undefined), false);
});

test("decodeClipboardHex decodes the osascript HTML clipboard form", () => {
  const decoded = decodeClipboardHex("«data HTML3C623E»");
  assert.equal(decoded.toString("utf8"), "<b>");
});

test("decodeClipboardHex rejects text that is not the HTML clipboard form", () => {
  assert.throws(() => decodeClipboardHex("missing"), /could not decode/);
  assert.throws(() => decodeClipboardHex("«data HTML3C6»"), /could not decode/);
});

test("cleanTypes keeps only the MIME type of a LibreOffice object descriptor", () => {
  const descriptor =
    'application/x-openoffice-objectdescriptor-xml;windows_formatname="Star Object Descriptor (XML)";' +
    'classname="com.sun.star.text.TextDocument";typename="LibreOffice 26.2 Text Document";' +
    'displayname="file:///home/someone/repos/x/sample.odt";viewaspect="1"';
  assert.deepEqual(cleanTypes([descriptor]), ["application/x-openoffice-objectdescriptor-xml"]);
});

test("cleanTypes normalises LibreOffice internal ids to N", () => {
  assert.deepEqual(cleanTypes(["application/x-libreoffice-internal-id-1809584"]), [
    "application/x-libreoffice-internal-id-N",
  ]);
});

test("cleanTypes removes duplicates while keeping the first-seen order", () => {
  assert.deepEqual(
    cleanTypes(["text/html", "text/plain;charset=utf-8", "text/plain", "text/plain;charset=utf-8", "image/png"]),
    ["text/html", "text/plain", "image/png"],
  );
});

test("cleanTypes output never contains local paths or machine details", () => {
  const cleaned = cleanTypes([
    "text/html",
    'application/x-openoffice-objectdescriptor-xml;displayname="file:///home/someone/sample.odt"',
    "application/x-libreoffice-internal-id-1809584",
    "text/plain;charset=utf-8",
    "text/plain",
  ]);
  const joined = cleaned.join("\n");
  assert.equal(joined.includes("file:"), false);
  assert.equal(joined.includes("/home/"), false);
  assert.deepEqual(cleaned, [
    "text/html",
    "application/x-openoffice-objectdescriptor-xml",
    "application/x-libreoffice-internal-id-N",
    "text/plain",
  ]);
});

test("buildMetadata cleans the clipboard types it records", () => {
  const metadata = buildMetadata({
    name: "libreoffice-writer",
    platform: "linux",
    session: "x11",
    types: ['application/x-openoffice-objectdescriptor-xml;displayname="file:///home/someone/sample.odt"', "text/plain;charset=utf-8", "text/plain"],
    htmlBytes: 1,
    textBytes: 1,
  });
  assert.deepEqual(metadata.types, ["application/x-openoffice-objectdescriptor-xml", "text/plain"]);
});

test("buildMetadata returns the documented shape with a date-only capturedAt", () => {
  const metadata = buildMetadata(
    {
      name: "word-online-firefox",
      app: "Word online",
      browser: "Firefox 130",
      note: null,
      platform: "linux",
      session: "wayland",
      types: ["text/html", "text/plain"],
      htmlBytes: 1234,
      textBytes: 56,
    },
    new Date("2026-11-12T15:30:00Z"),
  );
  assert.deepEqual(metadata, {
    name: "word-online-firefox",
    app: "Word online",
    browser: "Firefox 130",
    note: null,
    platform: "linux",
    session: "wayland",
    types: ["text/html", "text/plain"],
    capturedAt: "2026-11-12",
    htmlBytes: 1234,
    textBytes: 56,
  });
});

test("buildMetadata fills missing app, browser and note with null", () => {
  const metadata = buildMetadata({
    name: "web-page-chrome",
    platform: "darwin",
    session: "darwin",
    types: [],
    htmlBytes: 0,
    textBytes: 0,
  });
  assert.equal(metadata.app, null);
  assert.equal(metadata.browser, null);
  assert.equal(metadata.note, null);
  assert.match(metadata.capturedAt, /^\d{4}-\d{2}-\d{2}$/);
});
