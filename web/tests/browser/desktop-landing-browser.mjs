// There is no front page. A signed-out visitor, in a browser or the desktop
// app, goes straight to sign-in, unless the server has no sign-in, and signing
// out lands on the sign-in page marked as signed out instead of opening the
// marketing site.
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
    const providers = new URLSearchParams(location.search).has("none") ? [] : [{ id: 'github', kind: 'github', label: 'GitHub' }];
    globalThis.fetch = async (url, init) => {
      const path = String(url);
      // The desktop app's clone route: the bodies it was sent are kept across the
      // navigation that follows, and a cancelled chooser is asked for by the address.
      if (path.endsWith('/projects/clone')) {
        const sent = JSON.parse(sessionStorage.getItem('cloned') || '[]');
        sent.push(JSON.parse(init.body));
        sessionStorage.setItem('cloned', JSON.stringify(sent));
        if (sent.at(-1).url.includes('cancel')) return new Response(null, { status: 204 });
        return Response.json({ id: 'f9', project: 'local-f9', name: 'paper', root_name: 'paper' });
      }
      if (path === '/api/me') return Response.json(signedIn ? { name: 'Tester', can_publish: true, providers } : { providers });
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
    // The page a cloned folder opens on. The test only checks that the landing page went there.
    if (path === "/reader.html") { response.setHeader("content-type", "text/html"); response.end("<!doctype html><title>Reader</title><p id=reader>reader</p>"); return; }
    try { response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(join(out, path.split("/").pop()))); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const port = 31000 + Math.floor(Math.random() * 1000);
  tab = await browser("chromium", join(temp, "profile"), port);
  const at = () => tab.evaluate("location.pathname + location.search");

  // A browser sends a stranger to sign-in too.
  await tab.command("Page.navigate", { url: `${origin}/` });
  await until("sign-in page from a browser", () => tab.evaluate("location.pathname === '/auth/login'"));
  assert.equal(await at(), "/auth/login?next=%2F", "a browser opens sign-in");

  // The desktop app sends a stranger to sign-in, and back to the projects after.
  // The page replaces itself before it finishes loading, so no load event
  // belongs to this navigation; wait for the sign-in page instead.
  await tab.command("Page.navigate", { url: `${origin}/desktop/` });
  await until("sign-in page", () => tab.evaluate("location.pathname === '/auth/login'"));
  assert.equal(await at(), "/auth/login?next=%2F", "the desktop app opens sign-in");

  // A server with no sign-in has no sign-in page, so the app stays and says why.
  await tab.navigate(`${origin}/desktop/?none`);
  await until("no sign-in notice", () => tab.evaluate("document.body.innerText.includes('This server has no sign-in')"));
  assert.equal(await at(), "/desktop/?none", "the desktop app stays without providers");

  // Signed in, the desktop app shows the project browser.
  await tab.navigate(`${origin}/desktop/?in`);
  await until("project browser", () => tab.evaluate("Boolean(document.querySelector('.projects'))"));
  assert.equal(await at(), "/desktop/?in", "a signed-in desktop user stays on the project browser");
  assert.equal(await tab.evaluate("Boolean(document.querySelector('.open-link'))"), false, "the project browser has no link field");

  // Clone a repository sits beside Open folder in the desktop app. It asks for an
  // address, refuses one with a password before anything is sent, leaves the
  // dialog alone when the chooser is cancelled, and opens the cloned folder.
  const buttonLabels = () => tab.evaluate("[...document.querySelectorAll('button')].map((node) => node.textContent.trim())");
  const labels = await buttonLabels();
  assert.ok(labels.includes("Open folder…") && labels.includes("Clone a repository…"), "the desktop app offers Open folder and Clone a repository: " + labels);
  await tab.navigate(`${origin}/?in`);
  await until("project browser in a browser", () => tab.evaluate("Boolean(document.querySelector('.projects'))"));
  assert.equal((await buttonLabels()).includes("Clone a repository…"), false, "a browser has no Clone a repository");
  await tab.navigate(`${origin}/desktop/?in`);
  await until("project browser to clone from", () => tab.evaluate("[...document.querySelectorAll('button')].some((node) => node.textContent.trim() === 'Clone a repository…')"));
  const submitClone = (value) => tab.evaluate(`(() => {
    const input = document.querySelector('#clone-repository input');
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)});
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector('#clone-repository').requestSubmit();
  })()`);
  const cloned = () => tab.evaluate("JSON.parse(sessionStorage.getItem('cloned') || '[]')");
  await tab.evaluate("[...document.querySelectorAll('button')].find((node) => node.textContent.trim() === 'Clone a repository…').click()");
  await until("clone dialog", () => tab.evaluate("Boolean(document.querySelector('#clone-repository input'))"));
  await submitClone("https://you:secret@github.com/you/paper.git");
  await until("password refused", () => tab.evaluate("document.querySelector('#clone-repository')?.innerText.includes('password')"));
  assert.deepEqual(await cloned(), [], "an address with a password is not sent");
  await submitClone("https://cancel.example/paper.git");
  await until("cancelled chooser", async () => (await cloned()).length === 1);
  assert.equal(await at(), "/desktop/?in", "a cancelled chooser stays on the landing page");
  assert.equal(await tab.evaluate("Boolean(document.querySelector('#clone-repository input'))"), true, "and leaves the dialog open");
  await submitClone("git@github.com:you/paper.git");
  await until("cloned folder opened", () => tab.evaluate("location.pathname === '/reader.html' && location.search === '?local=f9'"));
  assert.deepEqual(await cloned(), [{ url: "https://cancel.example/paper.git" }, { url: "git@github.com:you/paper.git" }], "the addresses were sent as typed");

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

  // Signing out stays on the server, not the marketing site.
  await tab.evaluate("window.signOut()");
  await until("back at the sign-in page, marked as signed out", () => tab.evaluate("location.origin + location.pathname + location.search").then((here) => here === `${origin}/auth/login?signed_out=1`));
} finally {
  await tab?.close();
  server?.close();
  removeTemporary(temp);
}
console.log("desktop landing browser checks passed");
