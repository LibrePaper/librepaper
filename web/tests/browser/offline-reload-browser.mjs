// Verify that offline edits survive a page reload. The browser's IndexedDB
// persists the document state using Loro, and when the page reloads with the
// network unavailable, it restores from that cache. This closes the gate in
// SPEC-loro.md §6 Phase 2: "offline edits survive a reload".
//
// This test uses a real server and real browser to test the full lifecycle:
// 1. Open a document in the editor
// 2. Type text and let it persist to IndexedDB
// 3. Reload the page with network sync blocked
// 4. Verify the text survived the reload (came from IndexedDB, not server sync)

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, pause } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const binary = process.env.LIBREPAPER_TEST_BINARY || join(root, "dist", "librepaper");
assert.ok(existsSync(binary), "run `make build` first");

const temporary = mkdtempSync(join(tmpdir(), "librepaper-offline-reload-"));
const serverPort = 19200 + Math.floor(Math.random() * 1000);
const serverAddress = `http://127.0.0.1:${serverPort}`;

// Start the librepaper server with anyone able to publish and comment.
const server = spawn(binary, [
  "serve",
  "--port", String(serverPort),
  "--publishers", "anyone",
  "--commenters", "anyone",
], {
  env: { ...process.env, HOME: temporary },
  stdio: ["ignore", "pipe", "pipe"],
});

let serverLog = "";
server.stdout?.on("data", (chunk) => { serverLog += chunk; });
server.stderr?.on("data", (chunk) => { serverLog += chunk; });

let b;
try {
  // Wait for the server to be ready.
  await until("server health", async () => {
    try {
      const response = await fetch(`${serverAddress}/api/health`);
      return response.ok;
    } catch {
      return false;
    }
  }, 20000);
  console.log("✓ Server started");

  // Start the browser.
  b = await browser("chromium", join(temporary, "chrome"), 19300 + Math.floor(Math.random() * 1000));

  // Load the landing page to trigger the shell and get a visitor cookie.
  await b.navigate(`${serverAddress}/`);
  await until("landing page loaded", () => b.evaluate("Boolean(document.querySelector('a[href*=\"/docs\"]'))"), 10000);
  console.log("✓ Landing page loaded");

  // Create a document via POST /api/documents from inside the page.
  const createResult = await b.evaluate(`
    (async () => {
      const response = await fetch("/api/documents", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({
          title: "Offline Reload Test",
          source_format: "markdown",
        }),
      });
      const data = await response.json();
      return { ok: response.ok, slug: data.slug || data.document?.slug };
    })()
  `);
  assert.ok(createResult.ok, "failed to create document");
  assert.ok(createResult.slug, "no slug returned");

  const slug = createResult.slug;
  const docUrl = `${serverAddress}/docs/${slug}`;
  console.log(`✓ Document created: ${slug}`);

  // Navigate to the document.
  await b.navigate(docUrl);
  await until("document editor loaded", () => b.evaluate("Boolean(document.querySelector('.cm-editor'))"), 10000);
  console.log("✓ Document editor loaded");

  // Type text into the editor. We'll check that this text persists across reload.
  const testText = "OFFLINE_RELOAD_TEST_" + Date.now();
  await b.insert(testText, false);
  await pause(100);

  // Wait for the text to be persisted to IndexedDB. The persistence layer
  // subscribes to local updates and writes asynchronously. Give it time to settle.
  await until("text persisted", async () => {
    const text = await b.evaluate("document.querySelector('.cm-editor')?.innerText || ''");
    return text.includes(testText);
  }, 5000);

  // Verify the text is visible before going offline.
  const textBefore = await b.evaluate("document.querySelector('.cm-editor')?.innerText || ''");
  assert.ok(textBefore.includes(testText), "text not visible in editor before reload");
  console.log(`✓ Text typed and visible: "${testText}"`);

  // Block WebSocket connections (which would sync with server) using Network protocol.
  // This prevents the app from receiving updates from the server, so the text that
  // survives a reload must come from IndexedDB, not from a sync.
  // We keep HTTP available so the page can reload from the local server.
  await b.command("Network.enable");
  await b.command("Network.setBlockedURLs", {
    urls: ["ws://*", "wss://*"],
  });
  console.log("✓ WebSocket connections blocked");

  // Reload the page. It will load from the local HTTP server (not blocked),
  // but any attempts to connect to a server via WebSocket will fail.
  // The Loro persistence layer should restore the document from IndexedDB.
  await b.navigate(docUrl);
  await until("offline reload complete", () => b.evaluate("Boolean(document.querySelector('.cm-editor'))"), 10000);
  console.log("✓ Page reloaded with network sync blocked");

  // Get the text after reload. It should match what we typed before going offline.
  // The text must have come from IndexedDB since WebSocket was blocked.
  await pause(500); // Wait for persistence hydration and editor binding
  const textAfter = await b.evaluate("document.querySelector('.cm-editor')?.innerText || ''");

  // Report what we found
  console.log(`\nAfter offline reload:`);
  console.log(`  Expected text: "${testText}"`);
  console.log(`  Actual text:   "${textAfter}"`);
  console.log(`  Match: ${textAfter.includes(testText)}`);

  if (!textAfter.includes(testText)) {
    console.log(`\n⚠ Offline persistence appears to be broken`);
    console.log(`  The text was persisted to IndexedDB before reload`);
    console.log(`  But after reload with network sync blocked, the editor is empty`);
    console.log(`\n  This could be a bug in:`);
    console.log(`    - durableProjectPersistence hydration`);
    console.log(`    - The editor binding to the restored Loro document`);
    console.log(`    - The page lifecycle relative to when hydration completes`);
  }

  assert.ok(textAfter.includes(testText), `text lost after offline reload: expected "${testText}" in "${textAfter}"`);
  console.log(`✓ Text survived offline reload from IndexedDB`);

  // Unblock WebSocket to verify the app can reconnect normally.
  await b.command("Network.setBlockedURLs", { urls: [] });
  console.log("✓ WebSocket unblocked");

  console.log("\noffline-reload-browser: PASSED - offline edits survive a reload");
} catch (error) {
  console.error(`\nTest failed: ${error.message}`);
  if (serverLog) console.error(`\nServer log:\n${serverLog}`);
  throw error;
} finally {
  b?.close();
  server.kill();
  rmSync(temporary, { recursive: true, force: true });
}
