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
      if (path === '/api/me') return Response.json(signedIn ? { name: 'Tester', can_publish: true, providers: ['github'] } : { providers: ['github'] });
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

  // The desktop app sends a stranger to sign-in, and back to the projects after.
  await tab.navigate(`${origin}/desktop/`);
  await until("sign-in page", () => tab.evaluate("location.pathname === '/auth/login'"));
  assert.equal(await at(), "/auth/login?next=%2F", "the desktop app opens sign-in");

  // Signed in, the desktop app shows the project browser.
  await tab.navigate(`${origin}/desktop/?in`);
  await until("project browser", () => tab.evaluate("Boolean(document.querySelector('.projects'))"));
  assert.equal(await at(), "/desktop/?in", "a signed-in desktop user stays on the project browser");

  // Signing out in the desktop app stays on the server, not the marketing site.
  await tab.evaluate("window.signOut('https://example.invalid/')");
  await until("back at the root", () => tab.evaluate("location.origin + location.pathname").then((here) => here === `${origin}/`));
} finally {
  await tab?.close();
  server?.close();
  removeTemporary(temp);
}
console.log("desktop landing browser checks passed");
