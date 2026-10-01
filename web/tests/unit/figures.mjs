import assert from "node:assert/strict";
import { gather, release } from "../../src/lib/figures.js";

// Test that gather() creates data URLs for images and blob URLs for PDFs.
{
  const pngBytes = Uint8Array.of(0x89, 0x50, 0x4e, 0x47);
  const svgBytes = new TextEncoder().encode("<svg></svg>");
  const pdfBytes = Uint8Array.of(0x25, 0x50, 0x44, 0x46);

  // Stub fetch to return the bytes for each figure.
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (url.endsWith("/assets/a")) {
      return {
        ok: true,
        arrayBuffer: async () => pngBytes.buffer,
      };
    }
    if (url.endsWith("/assets/b")) {
      return {
        ok: true,
        arrayBuffer: async () => svgBytes.buffer,
      };
    }
    if (url.endsWith("/assets/c")) {
      return {
        ok: true,
        arrayBuffer: async () => pdfBytes.buffer,
      };
    }
    throw new Error(`Unexpected fetch: ${url}`);
  };

  // Stub URL.createObjectURL for the PDF test case.
  const originalCreateObjectURL = URL.createObjectURL;
  URL.createObjectURL = () => "blob:test-blob-url";

  try {
    const result = await gather("doc", {
      "fig.png": "a",
      "fig.svg": "b",
      "fig.pdf": "c",
    });

    // PNG should be a data URL starting with data:image/png;base64,
    const pngUrl = result.urls["fig.png"];
    assert.ok(pngUrl.startsWith("data:image/png;base64,"), "PNG URL starts with data:image/png;base64,");
    assert.ok(pngUrl.includes("#librepaper-asset=a"), "PNG URL includes fragment with asset digest");

    // SVG should be a data URL starting with data:image/svg+xml;base64,
    const svgUrl = result.urls["fig.svg"];
    assert.ok(svgUrl.startsWith("data:image/svg+xml;base64,"), "SVG URL starts with data:image/svg+xml;base64,");
    assert.ok(svgUrl.includes("#librepaper-asset=b"), "SVG URL includes fragment with asset digest");

    // PDF should be a blob URL
    const pdfUrl = result.urls["fig.pdf"];
    assert.ok(pdfUrl.startsWith("blob:"), "PDF URL starts with blob:");
    assert.ok(pdfUrl.includes("#librepaper-asset=c"), "PDF URL includes fragment with asset digest");

    // All assets should be present as bytes
    assert.deepEqual([...result.assets["fig.png"]], [...pngBytes]);
    assert.deepEqual([...result.assets["fig.svg"]], [...svgBytes]);
    assert.deepEqual([...result.assets["fig.pdf"]], [...pdfBytes]);

    // No missing figures
    assert.deepEqual(result.missing, []);

    // release() should be called to clean up blob URLs
    release();
  } finally {
    URL.createObjectURL = originalCreateObjectURL;
    globalThis.fetch = originalFetch;
  }
}

console.log("figures: data URLs for images, blob URLs for PDFs");
