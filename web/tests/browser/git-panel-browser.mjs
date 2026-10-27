// Browser regression check for the Git panel of a web project: the last push,
// and in the desktop app the connection to a repository: the connect form and
// its refusals, Pull and Push with the version name, the merge settled file by
// file, and the sign-in.
//
// The panel is drawn from 260 to 600 px wide without sideways scrolling.
//
// The desktop app and the server are stubbed rather than run: what is under
// test is the panel and its controllers, so the stand-ins follow the routes'
// contracts and record what they were asked. The controllers are the real ones.
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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-git-panel-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import Git from ${JSON.stringify(join(root, "web/src/components/reader/Git.svelte"))};
import { createWebGit } from ${JSON.stringify(join(root, "web/src/lib/reader/web-git.svelte.js"))};
import { createWebConnection } from ${JSON.stringify(join(root, "web/src/lib/reader/web-connection.svelte.js"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

// Each scene sets the state the stand-ins answer with and the answers a call
// gives in turn. A value with fail is thrown as the desktop app's refusal.
const scene = () => window.scene;
const record = (...entry) => scene().calls.push(entry);
const refusal = (failure) => Object.assign(new Error(failure.message || failure.body?.error || "The desktop app refused the request."), { name: "Refused", status: failure.status, body: failure.body });
const settle = (value) => { if (value?.fail) throw refusal(value.fail); return value; };

// The desktop app's connection of the web project: the status it keeps, and what
// the routes do to it. Pull and push answer what the scene sets; a connect that
// fails throws the refusal the desktop app gives for a repository linked elsewhere.
const connectionApi = {
  connectionStatus: async () => { record("connectionStatus"); return structuredClone(scene().connection); },
  connectRepository: async (project, url) => {
    record("connectRepository", url);
    if (scene().connectFails) throw refusal({ message: "this repository is linked to another LibrePaper project", status: 409, body: { reason: "linked" } });
    scene().connection = structuredClone(scene().afterConnect);
    return structuredClone(scene().connection);
  },
  pullConnection: async () => { record("pullConnection"); return settle(scene().pullAnswer); },
  pushConnection: async () => { record("pushConnection"); return settle(scene().pushAnswer); },
  resolveConnection: async (project, path, keep) => {
    record("resolveConnection", path, keep);
    const left = scene().connection.merging.conflicts.filter((entry) => entry !== path);
    scene().connection = { ...scene().connection, merging: left.length ? { side: "repository", conflicts: left } : null };
    return structuredClone(scene().connection);
  },
  loginConnection: async () => { record("loginConnection"); return { authorization_id: "auth-1", user_code: "ABCD-EFGH" }; },
  loginConnectionComplete: async (project, authorizationId) => {
    record("loginConnectionComplete", authorizationId);
    scene().connection = { ...scene().connection, signed_in: true };
    return structuredClone(scene().connection);
  },
  disconnectConnection: async () => {
    record("disconnectConnection");
    scene().connection = { connected: false, signed_in: scene().connection.signed_in };
    return structuredClone(scene().connection);
  },
};

const webApi = {
  history: async (slug, key, after) => {
    record("history", slug, key, after);
    if (scene().historyFails) throw new Error("this document's history is not readable");
    const page = scene().pages.find((entry) => entry.cursor === after);
    return { labels: structuredClone(page.labels), next: page.next };
  },
};

Object.defineProperty(navigator, "clipboard", {
  configurable: true, value: { writeText: async (value) => { window.copied = value; } },
});

let component;
let connection;
const destroy = () => { component?.$destroy(); component = null; connection?.dispose(); connection = null; };
const defaults = () => ({
  calls: [],
  connection: { connected: false, signed_in: true },
  pages: [{ cursor: null, next: null, labels: [] }],
});

const mountWeb = async (next = {}, props = {}) => {
  destroy();
  window.scene = { ...defaults(), ...next };
  connection = createWebConnection({
    api: connectionApi, project: "https://app.example/docs/paper",
    nameVersion: async (message) => { record("nameVersion", message); },
    signedIn: async () => scene().session !== false,
    approve: async (code) => { record("approve", code); },
    open: (url) => { record("open", url); },
    wait: async () => {},
  });
  component = createClassComponent({ component: Git, target: document.getElementById("panel"), props: {
    open: true, web: createWebGit({ api: webApi }), slug: "paper", linkKey: "",
    desktop: false, connection, ...props,
  } });
  await tick();
};

const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
window.ready = true;
`;

const checks = `
const check = (condition, message) => { if (!condition) throw new Error(message); };
const squash = (node) => node.innerText.replace(/\\s+/g, " ").trim();
const base = () => document.getElementById("panel");
const text = (where = base()) => squash(where);
const panel = () => document.querySelector(".git-panel");
const button = (label, where = base()) => [...where.querySelectorAll("button")].find((node) => node.textContent.trim() === label) ?? null;
const press = async (label, where = base()) => { const node = button(label, where); check(node, "missing button " + label + ": " + text(where)); node.click(); await flush(); };
const type = async (node, value) => { check(node, "missing field"); node.value = value; node.dispatchEvent(new Event("input", { bubbles: true })); await flush(); };
const field = (label, where = base()) => [...where.querySelectorAll("label")].find((node) => node.textContent.trim().startsWith(label))?.querySelector("input") ?? null;
const calls = (name) => window.scene.calls.filter((call) => call[0] === name);
const overflows = () => panel().scrollWidth > panel().clientWidth + 1;
const widths = async (what) => {
  for (const width of [260, 320, 400, 600]) {
    document.documentElement.style.setProperty("--panel-width", width + "px");
    await flush();
    check(!overflows(), "no sideways scrolling " + what + " at " + width + "px");
  }
  document.documentElement.style.removeProperty("--panel-width");
};

window.gitPanelCheck = async () => {
  const minutesAgo = (minutes) => new Date(Date.now() - minutes * 60000).toISOString();
  const push = (by, label, at) => ({ sha: "p1", seq: 7, at, by, label, why: "push" });
  const headings = () => [...panel().querySelectorAll("h3")].map((node) => node.textContent.trim()).join("|");
  const alert = () => panel().querySelector('[role="alert"]')?.textContent.trim() ?? "";

  // In the browser the panel shows the last push and says where to connect a repository. It shows no commands.
  await mountWeb({ pages: [{ cursor: null, next: null, labels: [
    { sha: "s2", seq: 9, at: minutesAgo(5), by: "Bob", label: "Saved", why: "cli" },
    push("Alice", "Reword the greeting", minutesAgo(20)),
  ] }] });
  await flush();
  check(panel().getAttribute("aria-label") === "Git", "the web panel is called Git too");
  check(headings() === "Repository", "one section in the browser: " + headings());
  check(text().includes("Last pushed by Alice, 20 min ago: Reword the greeting"), "it names the last push: " + text());
  check(calls("history").length === 1 && calls("history")[0][1] === "paper", "from the history");
  check(text().includes("To push and pull with git, open this project in the LibrePaper desktop app.") && text().includes("keep a GitHub repository in sync with this project through a GitHub Action"), "the browser says where to connect: " + text());
  check(!panel().querySelector("pre") && !button("Connect"), "the browser shows no commands and no connect form");
  const docs = [...panel().querySelectorAll("a")].find((node) => node.getAttribute("href") === "/docs/git.html");
  check(docs && docs.textContent.trim() === "Read the Git docs" && docs.target === "_blank" && docs.rel.includes("noopener"), "the docs are one link away");
  const action = [...panel().querySelectorAll("a")].find((node) => node.getAttribute("href") === "/docs/git.html#github-action");
  check(action && action.textContent.trim() === "GitHub Action", "and the GitHub Action is in the docs: " + text());
  await widths("in a web project");

  // No push yet, and a history that cannot be read.
  await mountWeb();
  await flush();
  check(text().includes("No push yet."), "a project nobody pushed to says so: " + text());
  await mountWeb({ historyFails: true });
  await flush();
  check(text().includes("The last push could not be read."), "a history that cannot be read is said: " + text());

  // In the desktop app and not connected, the panel connects the project: the address is checked and trimmed.
  const connectedStatus = { connected: true, signed_in: true, url: "https://github.com/you/paper.git", branch: "main", merging: null };
  await mountWeb({ connection: { connected: false, signed_in: true }, afterConnect: connectedStatus }, { desktop: true });
  await flush();
  check(headings() === "Repository|Connect a repository", "the desktop app adds the connect section: " + headings());
  check(text().includes("Connect this project to a git repository. Git uses your own sign-in."), "and says what it does: " + text());
  check(field("Repository address")?.placeholder === "https://github.com/owner/repo.git", "an address field with an example");
  await widths("with the connect form");

  await type(field("Repository address"), "https://user:secret@github.com/you/paper.git");
  await press("Connect");
  check(alert().includes("That address contains a password") && calls("connectRepository").length === 0, "an address with a password is refused here, and not sent: " + alert());

  await type(field("Repository address"), "   https://github.com/you/paper.git  ");
  await press("Connect");
  check(calls("connectRepository").length === 1 && calls("connectRepository")[0][1] === "https://github.com/you/paper.git", "the trimmed address is connected: " + JSON.stringify(calls("connectRepository")));
  check(headings() === "Repository|Connected repository" && text().includes("https://github.com/you/paper.git, branch main"), "and the connected repository is shown: " + text());

  // A connect the desktop app refuses says why, and the form stays.
  await mountWeb({ connection: { connected: false, signed_in: true }, connectFails: true }, { desktop: true });
  await flush();
  await type(field("Repository address"), "https://github.com/you/paper.git");
  await press("Connect");
  check(alert() === "this repository is linked to another LibrePaper project" && button("Connect") && !button("Connecting…"), "a refused connect shows its message: " + alert());
  await widths("with a refusal shown");

  // Connected and signed in: Pull writes what came in, and Push names the version before it sends it.
  await mountWeb({ connection: connectedStatus, afterConnect: connectedStatus, pullAnswer: { status: connectedStatus, pushed: { files: 2 } }, pushAnswer: { status: connectedStatus, pushed: "pushed" } }, { desktop: true });
  await flush();
  check(headings() === "Repository|Connected repository", "a connected project shows its repository: " + headings());
  check(text().includes("Pull brings the repository's commits into this project. Push names this version and sends it to the repository."), "and says what Pull and Push do: " + text());
  check(field("Version name")?.value === "Edits from LibrePaper", "the version name starts with its default");
  await widths("with a connected repository");

  await press("Pull");
  check(calls("pullConnection").length === 1 && text().includes("Pulled 2 files into the project."), "Pull says what came in: " + text());
  await type(field("Version name"), "Tighter abstract");
  await press("Push");
  check(calls("nameVersion").length === 1 && calls("nameVersion")[0][1] === "Tighter abstract", "the version is named first: " + JSON.stringify(calls("nameVersion")));
  check(calls("pushConnection").length === 1 && text().includes("Pushed to main."), "then the push is sent and says so: " + text());
  check(field("Version name").value === "Edits from LibrePaper", "the default name comes back after a push");

  await press("Disconnect");
  check(calls("disconnectConnection").length === 1 && headings() === "Repository|Connect a repository", "Disconnect forgets the copy and shows the connect form: " + headings());

  // Merging: the two files are settled one at a time, and Pull and Push wait until the merge is done.
  const merging = { ...connectedStatus, merging: { side: "repository", conflicts: ["intro.tex", "chapters/outline.tex"] } };
  await mountWeb({ connection: merging }, { desktop: true });
  await flush();
  check(text().includes("These files changed in both the project and the repository. Choose which version to keep."), "a merge says what to choose: " + text());
  check(button("Pull")?.disabled && button("Push")?.disabled, "while merging, Pull and Push wait: " + text());
  await press("Keep project's");
  check(calls("resolveConnection").length === 1 && calls("resolveConnection")[0][1] === "intro.tex" && calls("resolveConnection")[0][2] === "project", "Keep project's keeps the project's file: " + JSON.stringify(calls("resolveConnection")));
  check(!text().includes("The merge is done."), "the merge is not done while a file is left");
  await press("Take repository's");
  check(calls("resolveConnection").length === 2 && calls("resolveConnection")[1][1] === "chapters/outline.tex" && calls("resolveConnection")[1][2] === "repository", "Take repository's takes the repository's file: " + JSON.stringify(calls("resolveConnection")));
  check(text().includes("The merge is done. Pull or Push again."), "after the last file the merge is done: " + text());

  // Not signed in: the panel asks for the sign-in, which approves the code here.
  await mountWeb({ connection: { connected: true, signed_in: false, url: "https://github.com/you/paper.git", branch: "main", merging: null } }, { desktop: true });
  await flush();
  check(text().includes("Sign in so the desktop app can reach this project.") && !button("Pull"), "a project not signed in asks for the sign-in: " + text());
  await press("Sign in");
  check(calls("loginConnection").length === 1 && calls("approve")[0][1] === "ABCD-EFGH", "Sign in gets a code and approves it here: " + JSON.stringify(window.scene.calls));
  check(button("Pull") && !text().includes("Sign in so the desktop app can reach this project."), "and the repository is shown once signed in: " + text());

  check(!/[\\u2013\\u2014]/.test(document.getElementById("panel").innerText), "no dashes in the panel text");
  return true;
};
`;

writeFileSync(entry, `${source}${checks}`);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "git-panel-check.js", cssFileName: "style" } } });
  assertFreshDist("git-panel-browser");
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
      + '<body><div id="panel"></div><script type="module" src="/git-panel-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 24000 + Math.floor(Math.random() * 1000));
  await tab.resize(700, 900);
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("git panel component", () => tab.evaluate("Boolean(window.ready)"));
  assert.equal(await tab.evaluate("window.gitPanelCheck()"), true);
  console.log("git panel: last push and connection to a repository, narrow widths");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
