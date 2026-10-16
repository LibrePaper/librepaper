// The desktop app's sign-in page offers another server; a browser does not.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-signin-server-browser-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");
const bodies = [];
let server, tab;
try {
  writeFileSync(entry, `
    import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
    import SignIn from ${JSON.stringify(join(root, "web/src/components/SignIn.svelte"))};
    import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
    const realFetch = globalThis.fetch;
    globalThis.fetch = async (url, init) => {
      const path = String(url);
      if (path.includes('/__lp/')) return realFetch(url, init);
      if (path === '/api/me') return Response.json(new URLSearchParams(location.search).has('oidc') ? { providers: ['github', 'oidc'], oidc_name: 'Acme SSO' } : { providers: ['github'] });
      return Response.json({});
    };
    mount(SignIn, { target: document.body });
  `);
  await build({ configFile: false, root: join(root, "web"), plugins: [tailwindcss(), svelte()],
    build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "librepaper-web" } }, logLevel: "error" });
  const shell = (desktop) => `<!doctype html><html lang="en" data-theme="librepaper"><head><meta charset="utf-8"><link rel="stylesheet" href="/librepaper-web.css"></head>
    <body>${desktop ? '<script>window.__LIBREPAPER_DESKTOP__ = Object.freeze({ base: location.origin + "/__lp/", version: "test" })</script>' : ""}<script type="module" src="/check.js"></script></body></html>`;
  server = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    const path = url.pathname;
    if (path === "/" || path === "/desktop/") { response.setHeader("content-type", "text/html"); response.end(shell(path === "/desktop/")); return; }
    if (path === "/__lp/shell/server" && request.method === "POST") {
      let raw = "";
      request.on("data", (chunk) => { raw += chunk; });
      request.on("end", () => {
        const body = JSON.parse(raw || "{}");
        bodies.push(body);
        response.setHeader("content-type", "application/json");
        if (body.url === "https://bad") {
          response.statusCode = 400;
          response.end(JSON.stringify({ error: "Enter the server's address, starting with https://" }));
          return;
        }
        response.statusCode = 204;
        response.end();
      });
      return;
    }
    try { response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(join(out, path.split("/").pop()))); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const port = 31000 + Math.floor(Math.random() * 1000);
  tab = await browser("chromium", join(temp, "profile"), port);

  const setField = (value) => tab.evaluate(`(() => {
    const input = document.querySelector('.signin-server-form input');
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)});
    input.dispatchEvent(new Event('input', { bubbles: true }));
  })()`);
  const submitForm = () => tab.evaluate("document.querySelector('.signin-server-form').requestSubmit()");

  // A browser shows the providers and no way to pick a server.
  await tab.navigate(`${origin}/`);
  await until("github provider", () => tab.evaluate("Boolean(document.querySelector('a[href^=\"/auth/login/github\"]'))"));
  assert.equal(await tab.evaluate("Boolean(document.querySelector('details.signin-server'))"), false, "a browser offers no server choice");

  // A generic OpenID Connect provider is labelled with the deployment's name.
  await tab.navigate(`${origin}/?oidc`);
  await until("oidc provider", () => tab.evaluate("Boolean(document.querySelector('a[href^=\"/auth/login/oidc\"]'))"));
  assert.equal(await tab.evaluate("document.querySelector('a[href^=\"/auth/login/oidc\"]').textContent.trim()"), "Continue with Acme SSO");

  // The desktop app offers the choice and hands the address to the shell.
  await tab.navigate(`${origin}/desktop/`);
  await until("server choice", () => tab.evaluate("Boolean(document.querySelector('details.signin-server'))"));
  await tab.evaluate("document.querySelector('details.signin-server').open = true");
  await setField("https://paper.example.org");
  await submitForm();
  await until("server request", () => bodies.some((body) => body.url === "https://paper.example.org"));
  assert.deepEqual(bodies.find((body) => body.url === "https://paper.example.org"), { url: "https://paper.example.org" });

  // A bad address is refused by the shell and the reason is shown.
  await until("submit button enabled", () => tab.evaluate("!document.querySelector('.signin-server-form button').disabled"));
  await setField("https://bad");
  await submitForm();
  await until("refusal shown", () => tab.evaluate("(document.querySelector('.signin-server-error')?.textContent || '').includes('starting with https://')"));
} finally {
  await tab?.close();
  server?.close();
  removeTemporary(temp);
}
console.log("signin-server-browser: the desktop sign-in page offers another server; a browser does not");
