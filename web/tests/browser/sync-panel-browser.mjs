// Browser regression check for the Sync panel of a folder project: the states
// it moves through (signed out, signed in and not linked, linked), what Pull
// and Push say back, the files that changed on both sides with their two
// answers, and the refusals of a push.
//
// The desktop app is stubbed rather than run: what is under test is the panel
// and its controller, so the stand-in follows the sync routes' contract and
// records what it was asked. The controller is the real one.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync, existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { assertFreshDist } from "../helpers/fresh-dist.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-sync-panel-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import Sync from ${JSON.stringify(join(root, "web/src/components/reader/Sync.svelte"))};
import { createLocalSync } from ${JSON.stringify(join(root, "web/src/lib/reader/local-sync.svelte.js"))};
import { day } from ${JSON.stringify(join(root, "web/src/lib/dates.js"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

const linked = (extra = {}) => ({ signed_in: true, linked: true, site: "https://app.example/", slug: "paper", pulled: 4, changes: { here: 2, web: true }, ...extra });

// Each scene sets the status the stand-in answers with, what each command
// does to it, and the answers a pull or a push gives in turn. Signing in is
// the desktop app's code, the page's approval of it, and the finish, which
// refuses while the code is pending for as many tries as the scene asks. A
// scene with session false is a page with no session on the server: the code
// is then approved in another tab, and hold is the promise that the wait
// between two asks of the finish stays in until the scene releases it.
const scene = () => window.scene;
const refusal = (failure) => Object.assign(new Error(failure.message || failure.body?.error || "The desktop app refused the request."), { name: "Refused", status: failure.status, body: failure.body });
const settle = (value) => { if (value?.fail) throw refusal(value.fail); return value; };
const api = {
  syncStatus: async () => { scene().calls.push(["syncStatus"]); return structuredClone(scene().status); },
  syncLogin: async () => {
    scene().calls.push(["syncLogin"]);
    return { authorization_id: "auth-1", user_code: "ABCD-EFGH", ...(scene().verification ? { verification_url: scene().verification } : {}) };
  },
  syncLoginComplete: async (id, authorizationId) => {
    scene().calls.push(["syncLoginComplete", authorizationId]);
    if (scene().pendingTries > 0) {
      scene().pendingTries -= 1;
      throw refusal({ status: 400, body: { error: "sign in and approve the code, then retry" } });
    }
    if (scene().expired) throw refusal({ status: 400, body: { error: "that sign-in expired" } });
    scene().status = scene().afterLogin;
    return structuredClone(scene().status);
  },
  syncLink: async (id, url) => { scene().calls.push(["syncLink", url]); scene().status = scene().afterLink; },
  syncPublish: async (id, title) => { scene().calls.push(["syncPublish", title]); scene().status = scene().afterLink; },
  syncUnlink: async () => { scene().calls.push(["syncUnlink"]); scene().status = { signed_in: true, linked: false, git: Boolean(scene().status.git) }; },
  syncPull: async (id, choices) => { scene().calls.push(["syncPull", choices]); return settle(scene().pulls.shift()); },
  syncPush: async (id, message) => { scene().calls.push(["syncPush", message]); return settle(scene().pushes.shift()); },
};

let component;
const mount = async (next, props = {}) => {
  component?.$destroy();
  window.scene = { calls: [], saves: 0, pulls: [], pushes: [], ...next };
  const sync = createLocalSync({
    id: "f1", api,
    approve: async (code) => { scene().calls.push(["approve", code]); },
    signedIn: async () => scene().session !== false,
    open: (url) => { scene().calls.push(["open", url]); },
    wait: () => scene().hold ?? Promise.resolve(),
    flush: async () => { scene().saves += 1; return scene().saved !== false; },
  });
  component = createClassComponent({ component: Sync, target: document.getElementById("panel"), props: { sync, open: true, defaultTitle: "thesis", ...props } });
  await tick();
};
const today = () => day(new Date());
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
window.ready = true;
`;

const checks = `
const check = (condition, message) => { if (!condition) throw new Error(message); };
const squash = (node) => node.innerText.replace(/\\s+/g, " ").trim();
const text = () => squash(document.getElementById("panel"));
const panel = () => document.querySelector(".sync-panel");
const button = (label) => [...document.querySelectorAll("#panel button")].find((node) => node.textContent.trim() === label) ?? null;
const press = async (label) => { const node = button(label); check(node, "missing button " + label + ": " + text()); node.click(); await flush(); };
const type = async (node, value) => { node.value = value; node.dispatchEvent(new Event("input", { bubbles: true })); await flush(); };
const field = (label) => [...document.querySelectorAll("#panel label")].find((node) => node.textContent.trim().startsWith(label))?.querySelector("input");
const calls = (name) => window.scene.calls.filter((call) => call[0] === name);
const overflows = () => panel().scrollWidth > panel().clientWidth + 1;

window.syncPanelCheck = async () => {
  /* ------------------------------------------------------ signed out */

  await mount({ status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false } });
  await flush();
  check(text().includes("Sign in to keep this folder"), "a signed-out folder asks for a sign-in: " + text());
  check(!button("Pull") && !button("Link"), "and offers nothing else");
  await press("Sign in");
  check(calls("syncLogin").length === 1 && calls("approve")[0][1] === "ABCD-EFGH", "Sign in gets a code and approves it here: " + JSON.stringify(window.scene.calls));
  check(calls("syncLoginComplete")[0][1] === "auth-1", "and finishes with the authorization");
  check(text().includes("Link to a project"), "and the panel moves on to linking: " + text());

  // The finish refuses while the approval is still on its way, and the page asks again.
  await mount({ status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false }, pendingTries: 2 });
  await flush();
  await press("Sign in");
  check(calls("syncLoginComplete").length === 3 && text().includes("Link to a project"), "a pending code is tried again: " + text());

  // An expired sign-in says so and leaves the Sign in button.
  await mount({ status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false }, expired: true });
  await flush();
  await press("Sign in");
  check(text().includes("that sign-in expired") && button("Sign in"), "an expired sign-in goes back to the button: " + text());

  /* ----------------------------- no session on the server: another tab */

  // The page cannot approve the code, so it shows it, opens the server's page,
  // and waits until the code is approved there.
  const verification = "https://app.example/auth/device?code=ABCD-EFGH";
  let release;
  await mount({
    status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false },
    session: false, verification, hold: new Promise((resolve) => { release = resolve; }),
  });
  await flush();
  await press("Sign in");
  check(text().includes("Waiting for the code to be approved in the other tab"), "a page with no session waits for the other tab: " + text());
  check(panel().querySelector(".sync-code")?.textContent.trim() === "ABCD-EFGH", "and shows the code: " + text());
  check(calls("approve").length === 0, "it never approves the code itself");
  check(calls("open").length === 1 && calls("open")[0][1] === verification, "it opens the verification page the desktop app named: " + JSON.stringify(calls("open")));
  const again = [...document.querySelectorAll("#panel a")].find((node) => node.getAttribute("href") === verification);
  check(again && again.target === "_blank" && again.rel.includes("noopener"), "and offers the page as a link in case the tab was blocked");
  check(button("Cancel") && !button("Sign in") && !button("Cancel").disabled, "Cancel is there and Sign in is not: " + text());
  check(calls("syncLoginComplete").length === 0, "the finish has not been asked yet");
  for (const width of [260, 320, 400, 600]) {
    document.documentElement.style.setProperty("--panel-width", width + "px");
    await flush();
    check(!overflows(), "no sideways scrolling while waiting at " + width + "px");
  }
  document.documentElement.style.removeProperty("--panel-width");
  release();
  await flush();
  check(calls("syncLoginComplete").length === 1 && text().includes("Link to a project") && !text().includes("Waiting for the code"), "once the code is approved the panel moves on: " + text());

  // Cancel goes back to the Sign in button and asks the finish for nothing.
  await mount({
    status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false },
    session: false, hold: new Promise(() => {}),
  });
  await flush();
  await press("Sign in");
  check(calls("open")[0][1].endsWith("/auth/device?code=ABCD-EFGH"), "with no page named, the server's device page is opened: " + JSON.stringify(calls("open")));
  await press("Cancel");
  check(button("Sign in") && !button("Cancel") && !text().includes("Waiting for the code"), "Cancel goes back to the Sign in button: " + text());
  check(!text().includes("not approved") && calls("syncLoginComplete").length === 0, "and asks for nothing more: " + text());

  /* ------------------------------------------------ signed in, not linked */

  await mount({ status: { signed_in: true, linked: false }, afterLink: linked() });
  await flush();
  check(button("Link").disabled, "Link waits for an address");
  check(field("Or publish").value === "thesis", "the title offered is the folder's name");
  check(!button("Publish").disabled, "so Publish is ready");
  await type(field("Link to a project"), "  https://app.example/docs/paper  ");
  await press("Link");
  check(calls("syncLink")[0][1].trim() === "https://app.example/docs/paper", "Link sends the address: " + JSON.stringify(calls("syncLink")));
  check(text().includes("paper"), "and the linked project shows: " + text());

  await mount({ status: { signed_in: true, linked: false }, afterLink: linked({ changes: { here: 0, web: false } }) });
  await flush();
  await type(field("Or publish"), "A different title");
  await press("Publish");
  check(calls("syncPublish")[0][1] === "A different title", "Publish sends the title: " + JSON.stringify(calls("syncPublish")));

  /* --------------------------------------------------------------- linked */

  await mount({ status: linked() });
  await flush();
  const link = panel().querySelector(".sync-project a");
  check(link && link.getAttribute("href") === "https://app.example/docs/paper" && link.textContent.trim() === "paper", "the project is a link to its page: " + (link && link.getAttribute("href")));
  check(text().includes("2 changes here") && text().includes("The web project changed"), "the state is said in two lines: " + text());
  check(button("Pull") && button("Push") && button("Unlink"), "Pull, Push and Unlink are there");
  check(field("Message").value === today(), "the message starts as today's date: " + field("Message").value);

  await mount({ status: linked({ changes: { here: 0, web: false } }) });
  await flush();
  check(text().includes("No changes here") && text().includes("Up to date") && !text().includes("The web project changed"), "a quiet folder says so: " + text());
  await mount({ status: linked({ changes: { here: 1, web: false } }) });
  await flush();
  check(text().includes("1 change here"), "one change is not plural: " + text());

  /* ---------------------------------------------------- a git repository */

  const gitSentence = "This folder is a git repository. Sync it with git in a terminal, or with librepaper pull and push.";
  await mount({ status: { signed_in: false, linked: false, git: true } });
  await flush();
  check(text().includes(gitSentence), "a git folder says to use git: " + text());
  for (const label of ["Sign in", "Link", "Publish", "Pull", "Push", "Unlink"]) check(!button(label), label + " is not offered in a git folder: " + text());
  check(!text().includes("Link to a project"), "and there is no link form");

  await mount({ status: linked({ git: true }) });
  await flush();
  check(text().includes(gitSentence), "a linked git folder says it too: " + text());
  check(panel().querySelector(".sync-project a")?.getAttribute("href") === "https://app.example/docs/paper", "and still names its project");
  check(!button("Pull") && !button("Push") && !button("Publish") && !button("Link") && !field("Message"), "but offers neither pull nor push");
  check(!text().includes("changes here"), "and says nothing of changes");
  check(button("Unlink"), "Unlink stays");
  await press("Unlink");
  check(calls("syncUnlink").length === 1 && text().includes(gitSentence) && !panel().querySelector(".sync-project"), "Unlink leaves the sentence alone: " + text());

  /* ----------------------------------------------------------------- pull */

  await mount({
    status: linked(),
    pulls: [
      { written: ["a.tex", "b.tex"], trashed: ["old.tex"], kept: [], conflicts: ["intro.tex", "chapters/long-name.tex"], trimmed: false },
      { written: [], trashed: [], kept: [], conflicts: ["chapters/long-name.tex"] },
      { written: ["chapters/long-name.tex"], trashed: [], kept: [], conflicts: [] },
    ],
  });
  await flush();
  await press("Pull");
  check(window.scene.saves === 1, "the folder is saved before the pull");
  check(text().includes("Wrote 2 files and moved 1 file to the trash."), "the pull says what it did: " + text());
  check(text().includes("2 files changed here and on the web"), "and what needs an answer: " + text());
  const rows = () => [...document.querySelectorAll(".sync-conflict")];
  check(rows().length === 2 && rows()[0].textContent.includes("intro.tex"), "each file has a row");
  check(rows().every((row) => [...row.querySelectorAll("button")].map((node) => node.textContent.trim()).join("|") === "Keep mine|Take theirs"), "with the two answers");
  await press("Keep mine");
  check(JSON.stringify(calls("syncPull")[1][1]) === JSON.stringify({ mine: ["intro.tex"] }), "Keep mine settles that file: " + JSON.stringify(calls("syncPull")));
  check(rows().length === 1 && rows()[0].textContent.includes("long-name.tex"), "and the row goes");
  await press("Take theirs");
  check(JSON.stringify(calls("syncPull")[2][1]) === JSON.stringify({ theirs: ["chapters/long-name.tex"] }), "Take theirs settles the other");
  check(rows().length === 0 && text().includes("Wrote 3 files and moved 1 file to the trash."), "the counts add up: " + text());

  await mount({ status: linked({ changes: { here: 0, web: false } }), pulls: [{ written: [], trashed: [], kept: [], conflicts: [] }] });
  await flush();
  await press("Pull");
  check(text().includes("Nothing new on the web."), "a pull with nothing says so: " + text());
  await mount({ status: linked(), pulls: [{ written: ["a.tex"], trashed: [], conflicts: [], trimmed: true }] });
  await flush();
  await press("Pull");
  check(text().includes("was trimmed"), "a trimmed history is said: " + text());
  await mount({ status: linked(), pulls: [{ fail: { status: 401, body: { error: "sign in" }, message: "sign in" } }] });
  await flush();
  await press("Pull");
  check(text().includes("Sign in to the web project to continue.") && button("Sign in"), "a lost sign-in goes back to the Sign in button: " + text());

  /* ----------------------------------------------------------------- push */

  await mount({
    status: linked(),
    pushes: [
      { label: { sha: "abc", sequence: 5 }, files: 2 },
      { fail: { status: 409, body: { reason: "changed", paths: ["intro.tex", "fig/a.png"] } } },
      { fail: { status: 409, body: { reason: "suggestion", path: "intro.tex" } } },
      { nothing: true },
    ],
  });
  await flush();
  await type(field("Message"), "Fix the abstract");
  await press("Push");
  check(window.scene.saves === 1, "the folder is saved before the push");
  check(calls("syncPush")[0][1] === "Fix the abstract", "Push sends the message");
  check(text().includes("Pushed 2 files."), "and says what went: " + text());
  check(field("Message").value === today(), "the message goes back to the date for the next push: " + field("Message").value);
  await press("Push");
  check(calls("syncPush")[1][1] === today(), "an unnamed push is named by the date");
  check(text().includes("changed these files since your last pull") && text().includes("fig/a.png") && text().includes("Pull first."), "a push over changed files lists them and says to pull first: " + text());
  check(!text().includes("Pushed 2 files."), "the earlier result is gone");
  await press("Push");
  check(text().includes("pending suggestion in intro.tex") && text().includes("on the web"), "a suggestion is named and left to the web: " + text());
  check(!text().includes("Pull first."), "the earlier refusal is gone");
  await press("Push");
  check(text().includes("Nothing to push."), "an empty push says so: " + text());

  await mount({ status: linked(), saved: false });
  await flush();
  await press("Push");
  check(calls("syncPush").length === 0 && text().includes("could not be saved"), "a folder that cannot be saved stops the push: " + text());

  /* --------------------------------------------------------------- unlink */

  await mount({ status: linked() });
  await flush();
  await press("Unlink");
  check(calls("syncUnlink").length === 1 && text().includes("Link to a project"), "Unlink goes back to linking: " + text());

  /* ---------------------------------------------------------- narrow panel */

  await mount({
    status: linked({ slug: "a-project-with-a-name-long-enough-to-need-cutting-short" }),
    pulls: [{ written: ["a.tex"], trashed: [], conflicts: ["chapters/a-rather-long-folder-name/rendering-notes-for-the-appendix.qmd"] }],
    pushes: [{ fail: { status: 409, body: { reason: "changed", paths: ["chapters/a-rather-long-folder-name/rendering-notes-for-the-appendix.qmd"] } } }],
  });
  await flush();
  await press("Pull");
  await press("Push");
  for (const width of [260, 320, 400, 600]) {
    document.documentElement.style.setProperty("--panel-width", width + "px");
    await flush();
    check(!overflows(), "no sideways scrolling at " + width + "px");
  }
  check(!/[\\u2013\\u2014]/.test(text()), "no dashes in the panel text");
  return true;
};
`;

writeFileSync(entry, `${source}${checks}`);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "sync-panel-check.js", cssFileName: "style" } } });
  assertFreshDist("sync-panel-browser");
  // The built shell's stylesheet carries the theme every colour and step of
  // spacing the panel uses.
  const built = join(root, "web/dist/assets");
  const theme = readdirSync(built)
    .filter((name) => name.endsWith(".css"))
    .map((name) => readFileSync(join(built, name), "utf8"))
    .filter((text) => text.includes("--color-sidebar"))
    .join("\n");
  if (!theme) throw new Error("no built theme stylesheet: run `bun run build` in web/ first");
  server = createServer((request, response) => {
    if (request.url === "/theme.css") {
      response.setHeader("Content-Type", "text/css");
      return response.end(theme);
    }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", request.url.endsWith(".css") ? "text/css" : "text/javascript");
      return response.end(readFileSync(file));
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><html data-theme="librepaper"><head><meta charset="utf-8">'
      + '<link rel="stylesheet" href="/theme.css"><link rel="stylesheet" href="/style.css">'
      + '<style>body{margin:0;background:var(--color-sidebar)}#panel{width:var(--panel-width,320px);display:flex;flex-direction:column}</style></head>'
      + '<body><div id="panel"></div><script type="module" src="/sync-panel-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 24000 + Math.floor(Math.random() * 1000));
  await tab.resize(700, 900);
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("sync panel component", () => tab.evaluate("Boolean(window.ready)"));
  assert.equal(await tab.evaluate("window.syncPanelCheck()"), true);
  console.log("sync panel: sign-in, link and publish, linked state, pull with conflicts, push and its refusals, unlink");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
