// The desktop app has no front page. A signed-out visitor goes straight to
// sign-in, and signing out stays on the server instead of opening the
// marketing site. A browser keeps the signed-out landing page.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-desktop-landing-browser-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");
let server, tab;
try {
  writeFileSync(entry, `
    import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
    import Landing from ${JSON.stringify(join(root, "web/src/components/Landing.svelte"))};
    import { signOut } from ${JSON.stringify(join(root, "web/src/lib/api.js"))};
    import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
    const signedIn = new URLSearchParams(location.search).has("in");
    globalThis.fetch = async (url) => {
      const path = String(url);
      if (path === '/api/me') return Response.json(signedIn ? { name: 'Tester', can_publish: true, providers: [{ id: 'github', kind: 'github', label: 'GitHub' }] } : { providers: [{ id: 'github', kind: 'github', label: 'GitHub' }] });
      if (path.startsWith('/api/list')) return Response.json({ documents: [] });
      return Response.json({});
    };
    window.signOut = signOut;
    mount(Landing, { target: document.body });
  `);
  await build({ configFile: false, root: join(root, "web"), plugins: [tailwindcss(), svelte()],
    build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "librepaper-web" } }, logLevel: "error" });
  const shell = (desktop) => `<!doctype html><html lang="en" data-theme="librepaper"><head><meta charset="utf-8"><link rel="stylesheet" href="/librepaper-web.css"></head>
    <body>${desktop ? '<script>window.__LIBREPAPER_DESKTOP__ = Object.freeze({ base: location.origin + "/__lp/", version: "test" })</script>' : ""}<script type="module" src="/check.js"></script></body></html>`;
  server = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    const path = url.pathname;
    if (path === "/" || path === "/desktop/") { response.setHeader("content-type", "text/html"); response.end(shell(path === "/desktop/")); return; }
    if (path === "/auth/login") { response.setHeader("content-type", "text/html"); response.end("<!doctype html><title>Sign in</title><p id=signin>sign in</p>"); return; }
    try { response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(join(out, path.split("/").pop()))); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const port = 31000 + Math.floor(Math.random() * 1000);
  tab = await browser("chromium", join(temp, "profile"), port);
  const at = () => tab.evaluate("location.pathname + location.search");

  // A browser keeps the signed-out landing page.
  await tab.navigate(`${origin}/`);
  await until("signed-out landing", () => tab.evaluate("Boolean(document.querySelector('a[href^=\"/auth/login\"]'))"));
  assert.equal(await at(), "/", "a browser stays on the landing page");
  assert.equal(await tab.evaluate("Boolean(document.querySelector('.open-link'))"), false, "a browser has no link field");

  // The desktop app sends a stranger to sign-in, and back to the projects after.
  // The page replaces itself before it finishes loading, so no load event
  // belongs to this navigation; wait for the sign-in page instead.
  await tab.command("Page.navigate", { url: `${origin}/desktop/` });
  await until("sign-in page", () => tab.evaluate("location.pathname === '/auth/login'"));
  assert.equal(await at(), "/auth/login?next=%2F", "the desktop app opens sign-in");

  // Signed in, the desktop app shows the project browser.
  await tab.navigate(`${origin}/desktop/?in`);
  await until("project browser", () => tab.evaluate("Boolean(document.querySelector('.projects'))"));
  assert.equal(await at(), "/desktop/?in", "a signed-in desktop user stays on the project browser");
  assert.equal(await tab.evaluate("Boolean(document.querySelector('.open-link'))"), false, "the project browser has no link field");

  // The desktop app opens a pasted link from its own server, keys and all, and
  // refuses one from anywhere else.
  const pasteLink = (value) => tab.evaluate(`(() => {
    const input = document.querySelector('.open-link input');
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)});
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector('.open-link').requestSubmit();
  })()`);
  await tab.navigate(`${origin}/desktop/?in&new=link`);
  await until("open a link", () => tab.evaluate("Boolean(document.querySelector('.open-link input'))"));
  await pasteLink("https://elsewhere.example/docs/x#k=secret");
  await until("other server refused", () => tab.evaluate("document.querySelector('.open-link-error')?.textContent.includes('Paste a link from')"));
  await pasteLink(`${origin}/desktop/?in&opened#k=abc`);
  await until("pasted link opened", () => tab.evaluate("location.search === '?in&opened' && location.hash === '#k=abc'"));
  await until("project browser again", () => tab.evaluate("typeof window.signOut === 'function' && Boolean(document.querySelector('.projects'))"));

  // Signing out in the desktop app stays on the server, not the marketing site.
  await tab.evaluate("window.signOut('https://example.invalid/')");
  await until("back at the sign-in page, marked as signed out", () => tab.evaluate("location.origin + location.pathname + location.search").then((here) => here === `${origin}/auth/login?signed_out=1`));
} finally {
  await tab?.close();
  server?.close();
  removeTemporary(temp);
}
console.log("desktop landing browser checks passed");
