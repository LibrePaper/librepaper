// The Git panel's connected view over stand-ins for the sync code: the
// identity line, the status row, the local changes and Commit & push, which
// pushes only a list the person saw, at sidebar widths from 260px to 600px.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { existsSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-git-panel-browser-"));
const output = join(temporary, "build");
const imp = (path) => JSON.stringify(join(root, path));
const entry = join(temporary, "entry.js");
const mocks = { sync: join(temporary, "sync.js"), bridge: join(temporary, "bridge.js"), server: join(temporary, "server.js") };

// Every answer comes from `window.git`, which the test sets per scene.
writeFileSync(mocks.sync, `
const scene = () => window.git;
const record = (name, args) => { scene().calls.push([name, ...args]); };
export async function status() { return { git: { supported: true, git: "2.47.0", connected: true, connection_id: "c1", remote: scene().gitRemote ?? { url: "https://github.com/vincentarelbundock/a-repository-with-a-long-name.git" } }, link: scene().link, pending: [], conflict: scene().conflict ?? null }; }
export async function checkRemote() { record("checkRemote", []); return scene().remote; }
export async function remoteChanges() { record("remoteChanges", []); return scene().incoming ?? { status: "none", head: null, files: [] }; }
export async function localChanges() { record("localChanges", []); return scene().local; }
export async function outgoing() { record("outgoing", []); return scene().outgoing; }
export async function push(ctx, message, options) { record("push", [message, options]); scene().local = { status: "none", files: [] }; return { status: "pushed", commit: "abcdef1234" }; }
export async function pushToBranch(ctx, branch, message) { record("pushToBranch", [branch, message]); return scene().branched ?? { status: "branched", branch, commit: "c9", files: [{ path: "librepaper.md", status: "M" }] }; }
export async function pull() { record("pull", []); return { status: "nothing", files: [] }; }
export async function recover() { return { status: "done", pending: [] }; }
export async function connect(ctx, draft) { record("connect", [draft]); scene().link = scene().connectLink ?? scene().link; return { status: "connected", link: scene().link }; }
export async function disconnect() { record("disconnect", []); }
export async function discardConflict(ctx, session) { record("discardConflict", [session?.id]); scene().conflict = null; return { status: "nothing" }; }
export async function leaveBranch() { record("leaveBranch", []); scene().link = null; }
export function defaultMessage(files) { return "Update " + files.map((file) => file.path).join(", "); }
`);
writeFileSync(mocks.bridge, `
const scene = () => window.git;
export function createBridge() {
  return {
    branches: async (remote) => { scene().calls.push(["branches", remote]); return { branches: ["main", "draft"], default: "main" }; },
  };
}
`);
writeFileSync(mocks.server, "export function syncApi() { return {}; }\n");

writeFileSync(entry, `
window.__LIBREPAPER_DESKTOP__ = Object.freeze({ base: "lp-local://localhost/", version: "test", zoom: 1 });
import { tick } from ${imp("node_modules/svelte/src/index-client.js")};
import { createClassComponent } from ${imp("node_modules/svelte/src/legacy/legacy-client.js")};
window.testErrors=[];addEventListener("error",e=>window.testErrors.push(e.message));addEventListener("unhandledrejection",e=>window.testErrors.push(String(e.reason)));
const { default: Git } = await import(${imp("src/components/reader/Git.svelte")});
let component;
window.mount = async (scene) => {
  component?.$destroy();
  window.git = { calls: [], ...scene };
  component = createClassComponent({ component: Git, target: document.getElementById("panel"), props: {
    open: true, slug: "paper", session: { onFiles: () => () => {}, list: () => [] }, rules: {}, me: { name: "Ada" },
    resolver: { state: { active: false }, open() {}, async discard() {} },
    differ: { state: { active: false, selected: "" }, show(ctx, meta, path) { window.git.calls.push(["show", path]); } },
  } });
  await tick();
};
const panel = () => document.querySelector(".gh-panel");
window.text = () => panel()?.innerText ?? "";
window.button = (label) => [...panel().querySelectorAll("button")].find((node) => node.textContent.trim() === label) ?? null;
window.click = (label) => window.button(label).click();
window.overflows = () => { const node = panel(); return node.scrollWidth > node.clientWidth + 1; };
window.setMessage = (value) => { const input = panel().querySelector("input[type=text]"); input.value = value; input.dispatchEvent(new Event("input", { bubbles: true })); };
window.ready = true;
`);

const changes = [{ path: "librepaper.md", status: "M" }, { path: "sections/a-rather-long-folder-name/rendering-notes-for-the-appendix.qmd", status: "A" }];
const link = {
  connection_id: "c1", remote_kind: "network", remote_label: "https://github.com/vincentarelbundock/a-repository-with-a-long-name.git",
  branch: "main", folder: "", connected_by: { name: "Ada" }, last_pull_at: "2026-10-22T13:54:17Z", last_pull_by: { name: "Ada" },
};

let server;
let page;
try {
  await build({ configFile: false, root, plugins: [svelte()], logLevel: "error",
    resolve: { alias: [
      { find: /^\.\.\/\.\.\/lib\/gitsync\/sync\.js$/, replacement: mocks.sync },
      { find: /^\.\.\/\.\.\/lib\/gitsync\/bridge\.js$/, replacement: mocks.bridge },
      { find: /^\.\.\/\.\.\/lib\/gitsync\/server\.js$/, replacement: mocks.server },
    ] },
    build: { outDir: output, emptyOutDir: true, target: "es2022", lib: { entry, formats: ["es"], fileName: () => "git-panel-check.js", cssFileName: "style" } } });
  // The built shell's stylesheet carries the theme the panel uses.
  const built = join(root, "dist/assets");
  const theme = existsSync(built) ? readdirSync(built)
    .filter((name) => name.endsWith(".css"))
    .map((name) => readFileSync(join(built, name), "utf8"))
    .filter((text) => text.includes("--color-sidebar"))
    .join("\n") : "";
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
      + '<style>body{margin:0;background:var(--color-sidebar)}#panel{width:var(--panel-width,320px)}</style></head>'
      + '<body><div id="panel"></div><script type="module" src="/git-panel-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.resize(700, 700);
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("panel test loaded", () => page.evaluate("window.ready"), 10000);
  const evaluate = (code) => page.evaluate(code);
  const width = (px) => evaluate(`document.documentElement.style.setProperty("--panel-width", "${px}px")`);
  const shoot = process.env.LIBREPAPER_SHOTS
    ? async (name) => {
      const { data } = await page.command("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(process.env.LIBREPAPER_SHOTS, `${name}.png`), Buffer.from(data, "base64"));
    }
    : async () => {};

  // Up to date, with two local changes: the identity names owner/repo, the
  // row says what was checked, and the files are listed.
  const scene = { link, remote: { status: "current", head: "h1" }, local: { status: "changes", files: changes },
    outgoing: { status: "outgoing", files: changes, outgoing: { files: changes } } };
  await evaluate(`mount(${JSON.stringify(scene)})`);
  await until("up to date", () => evaluate('text().includes("Up to date")'), 5000);
  await until("changes listed", () => evaluate('text().includes("rendering-notes-for-the-appendix.qmd")'), 5000);
  assert.match(await evaluate("text()"), /vincentarelbundock\/a-repository-with-a-long-name/);
  assert.match(await evaluate("text()"), /Local changes/);
  assert.equal(await evaluate('button("Review push")'), null);
  for (const px of [260, 320, 400, 600]) {
    await width(px);
    assert.equal(await evaluate("overflows()"), false, `no horizontal overflow at ${px}px`);
    await shoot(`panel-${px}`);
  }
  await width(320);
  await evaluate('document.documentElement.dataset.mode = "dark"');
  await shoot("panel-320-dark");
  await evaluate("delete document.documentElement.dataset.mode");

  // The repository name opens its details and actions, and Disconnect asks first.
  await evaluate('document.querySelector(".gh-panel [aria-label=\'Repository details and actions\']").click()');
  const item = (label) => `[...document.querySelectorAll("[role=menuitem]")].find((node) => node.textContent.trim() === ${JSON.stringify(label)})`;
  await until("menu on screen", () => evaluate(`(() => { const node = ${item("Disconnect…")}; const box = node?.getBoundingClientRect(); return Boolean(box && box.width && box.top >= 0 && box.left >= 0); })()`), 5000);
  assert.match(await evaluate('document.body.innerText'), /Last pull/);
  await shoot("details-menu");
  // A repository on a known host opens in the browser from the menu.
  assert.ok(await evaluate(`Boolean(${item("Open in browser")})`), "a GitHub repository can be opened");
  // The details hang below the branch line, not over it.
  assert.equal(await evaluate('document.querySelector(".gh-details").getBoundingClientRect().top >= document.querySelector(".gh-panel .gh-branch").getBoundingClientRect().bottom'), true, "the menu leaves the branch visible");
  assert.equal(await evaluate('Boolean(document.querySelector(".gh-panel [aria-label=\'Connection details\']"))'), false, "the ellipsis menu is gone");
  // Zag selects a menu item on a pointer release, so the click is a real one.
  const box = await evaluate(`(() => { const r = ${item("Disconnect…")}.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
    await page.command("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "left", clickCount: 1 });
  }
  await until("confirmation", () => evaluate('document.body.innerText.includes("Sync stops until you connect again")'), 5000);
  assert.equal(await evaluate('git.calls.some((call) => call[0] === "disconnect")'), false);
  await evaluate('[...document.querySelectorAll("[role=dialog] button")].find((node) => node.textContent.trim() === "Cancel").click()');
  await until("confirmation closed", () => evaluate('!document.body.innerText.includes("Sync stops until you connect again")'), 5000);

  // Push to branch asks for a name, sends the local changes as one commit to
  // that new branch and closes. A name the remote already has keeps the dialog
  // open and says so.
  const openPushToBranch = async () => {
    await evaluate('document.querySelector(".gh-panel [aria-label=\'Repository details and actions\']").click()');
    await until("push to branch on screen", () => evaluate(`(() => { const node = ${item("Push to branch…")}; const box = node?.getBoundingClientRect(); return Boolean(box && box.width && box.top >= 0 && box.left >= 0); })()`), 5000);
    const pushBox = await evaluate(`(() => { const r = ${item("Push to branch…")}.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
      await page.command("Input.dispatchMouseEvent", { type, x: pushBox.x, y: pushBox.y, button: "left", clickCount: 1 });
    }
    await until("push to branch dialog", () => evaluate('document.body.innerText.includes("Push to a new branch")'), 5000);
  };
  const typeBranch = (name) => evaluate(`(() => { const input = document.querySelector("#push-branch input"); input.value = ${JSON.stringify(name)}; input.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  await openPushToBranch();
  await typeBranch("review/intro");
  await evaluate('document.getElementById("push-branch").requestSubmit()');
  await until("branch pushed", () => evaluate('git.calls.some((call) => call[0] === "pushToBranch")'), 5000);
  assert.deepEqual(await evaluate('git.calls.find((call) => call[0] === "pushToBranch")'), ["pushToBranch", "review/intro", ""]);
  await until("branch dialog closed", () => evaluate('!document.body.innerText.includes("Push to a new branch")'), 5000);

  // A branch name the remote already has is refused: the dialog stays and says why.
  await evaluate('window.git.branched = { status: "refused", exists: true, refusals: [{ reason: "A branch named review/intro already exists on the remote. Choose another name." }] }');
  await openPushToBranch();
  await typeBranch("review/intro");
  await evaluate('document.getElementById("push-branch").requestSubmit()');
  await until("branch exists", () => evaluate('document.body.innerText.includes("already exists")'), 5000);
  assert.equal(await evaluate('document.body.innerText.includes("Push to a new branch")'), true, "the dialog stays open");
  await evaluate('[...document.querySelectorAll("[role=dialog] button")].find((node) => node.textContent.trim() === "Cancel").click()');
  await until("branch dialog cancelled", () => evaluate('!document.body.innerText.includes("Push to a new branch")'), 5000);

  // Commit & push with the list unchanged pushes the typed message once and
  // then shows that nothing is left.
  await evaluate('setMessage("Edit the introduction")');
  await evaluate('click("Commit & push")');
  await until("pushed", () => evaluate('git.calls.some((call) => call[0] === "push")'), 5000);
  const pushed = await evaluate('git.calls.find((call) => call[0] === "push")');
  assert.equal(pushed[1], "Edit the introduction");
  await until("nothing left", () => evaluate('text().includes("No local changes")'), 5000);
  assert.equal(await evaluate('button("Commit & push")'), null);

  // Commit & push stops when the remote has new commits, before pushing, and
  // offers Pull.
  await evaluate(`mount(${JSON.stringify({ ...scene, outgoing: { status: "behind", head: "h2", reason: "The remote has new commits. Pull first, then push." } })})`);
  await until("changes listed", () => evaluate('text().includes("rendering-notes-for-the-appendix.qmd")'), 5000);
  await evaluate('click("Commit & push")');
  await until("pull first", () => evaluate('text().includes("Pull before pushing")'), 5000);
  assert.equal(await evaluate('git.calls.some((call) => call[0] === "push")'), false);
  assert.ok(await evaluate('button("Pull") !== null'));

  // A remote with new commits disables Commit & push while local changes wait.
  await evaluate(`mount(${JSON.stringify({ ...scene, remote: { status: "behind", head: "h2" }, incoming: { status: "changes", head: "h2", files: [{ path: "librepaper.md", status: "M" }, { path: "references.bib", status: "M" }] } })})`);
  await until("changes listed again", () => evaluate('text().includes("rendering-notes-for-the-appendix.qmd")'), 5000);
  await until("remote changes listed", () => evaluate('text().includes("references.bib")'), 5000);
  assert.match(await evaluate("text()"), /Remote changes/);
  assert.ok(await evaluate('button("Pull") !== null'));
  assert.equal(await evaluate('button("Commit & push").disabled'), true);

  // New commits on the remote offer Pull; a failed check offers Retry.
  await evaluate(`mount(${JSON.stringify({ ...scene, remote: { status: "behind", head: "h2" }, local: { status: "none", files: [] },
    incoming: { status: "changes", head: "h2", files: [{ path: "librepaper.md", status: "M" }, { path: "references.bib", status: "M" }] } })})`);
  await until("behind", () => evaluate('text().includes("New commits on the remote") && text().includes("references.bib")'), 5000);
  assert.ok(await evaluate('button("Pull") !== null'));
  await shoot("panel-behind");
  await evaluate(`mount(${JSON.stringify({ ...scene, remote: { status: "refused", refusals: [{ reason: "Git could not reach the remote." }] } })})`);
  await until("unknown", () => evaluate('text().includes("Git could not reach the remote.")'), 5000);
  assert.ok(await evaluate('button("Retry") !== null'));

  // A sign-in Git could not make shows the steps for the remote. Try again
  // runs the check once more, and a passing check clears the steps.
  await evaluate(`mount(${JSON.stringify({ ...scene, remote: { status: "refused", kind: "auth", refusals: [{ reason: "Git could not sign in." }] } })})`);
  await until("sign-in steps", () => evaluate('text().includes("gh auth setup-git")'), 5000);
  assert.ok(await evaluate('button("Try again") !== null'));
  await width(260);
  assert.equal(await evaluate("overflows()"), false, "no horizontal overflow at 260px");
  await evaluate('window.git.remote = { status: "current", head: "h1" }');
  await evaluate('click("Try again")');
  await until("up to date again", () => evaluate('text().includes("Up to date") && !text().includes("gh auth setup-git")'), 5000);

  // A conflicted pull offers Cancel, which discards the session and clears the
  // line once the check passes again.
  await evaluate(`mount(${JSON.stringify({ ...scene, conflict: { id: "s1", connection_id: "c1", conflicts: [] } })})`);
  await until("conflict line", () => evaluate('text().includes("Conflicts need resolution")'), 5000);
  assert.ok(await evaluate('button("Resolve") !== null'));
  assert.ok(await evaluate('button("Cancel") !== null'));
  await evaluate('click("Cancel")');
  await until("conflict discarded", () => evaluate('git.calls.some((call) => call[0] === "discardConflict" && call[1] === "s1")'), 5000);
  await until("conflict line cleared", () => evaluate('!text().includes("Conflicts need resolution") && text().includes("Up to date")'), 5000);

  // Switch branch confirms first, then ends the connection, lists the branches
  // of the same remote and fills the setup form with its URL.
  await evaluate(`mount(${JSON.stringify(scene)})`);
  await until("up to date for switch", () => evaluate('text().includes("Up to date")'), 5000);
  await evaluate('document.querySelector(".gh-panel [aria-label=\'Repository details and actions\']").click()');
  await until("switch on screen", () => evaluate(`(() => { const node = ${item("Switch branch…")}; const box = node?.getBoundingClientRect(); return Boolean(box && box.width && box.top >= 0 && box.left >= 0); })()`), 5000);
  const switchBox = await evaluate(`(() => { const r = ${item("Switch branch…")}.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
    await page.command("Input.dispatchMouseEvent", { type, x: switchBox.x, y: switchBox.y, button: "left", clickCount: 1 });
  }
  await until("switch confirmation", () => evaluate('document.body.innerText.includes("stops syncing with")'), 5000);
  assert.equal(await evaluate('git.calls.some((call) => call[0] === "leaveBranch")'), false);
  await evaluate('[...document.querySelectorAll("[role=dialog] button")].find((node) => node.textContent.trim() === "Switch").click()');
  await until("left branch", () => evaluate('git.calls.some((call) => call[0] === "leaveBranch") && git.calls.some((call) => call[0] === "branches")'), 5000);
  const listed = await evaluate('git.calls.find((call) => call[0] === "branches")[1]');
  assert.deepEqual(listed, { url: "https://github.com/vincentarelbundock/a-repository-with-a-long-name.git" });
  await until("branch options", () => evaluate('document.querySelector(".gh-panel select option[value=\'draft\']") !== null'), 5000);
  assert.equal(await evaluate('document.querySelector(".gh-panel input.gh-input[placeholder=\'HTTPS or SSH URL\']").value'), "https://github.com/vincentarelbundock/a-repository-with-a-long-name.git");

  // There is no Diff button. Clicking a local change opens its diff for that file.
  await evaluate(`mount(${JSON.stringify(scene)})`);
  await until("up to date for diff", () => evaluate('text().includes("Up to date")'), 5000);
  assert.equal(await evaluate('button("Diff")'), null);
  await evaluate(`document.querySelector(".gh-panel .gh-change-open[title='${changes[0].path}']").click()`);
  await until("file shown", () => evaluate('git.calls.some((call) => call[0] === "show")'), 5000);
  assert.equal(await evaluate('git.calls.find((call) => call[0] === "show")[1]'), changes[0].path);

  // Connect has no Compare step: a typed remote lists its branches, Connect
  // records the draft once, and neither pull nor push runs.
  await evaluate(`mount(${JSON.stringify({ link: null })})`);
  await until("setup form", () => evaluate('text().includes("Sync this project with a Git repository.")'), 5000);
  const typed = "https://github.com/vincentarelbundock/typed-repository.git";
  await evaluate(`(() => { const input = document.querySelector(".gh-panel input.gh-input[placeholder='HTTPS or SSH URL']"); input.value = ${JSON.stringify(typed)}; input.dispatchEvent(new Event("input", { bubbles: true })); input.dispatchEvent(new Event("change", { bubbles: true })); })()`);
  await until("branch options for typed remote", () => evaluate('document.querySelector(".gh-panel select option[value=\'draft\']") !== null'), 5000);
  assert.equal(await evaluate('button("Compare")'), null);
  await evaluate('click("Connect")');
  await until("connected", () => evaluate('git.calls.some((call) => call[0] === "connect")'), 5000);
  const connects = await evaluate('git.calls.filter((call) => call[0] === "connect")');
  assert.equal(connects.length, 1);
  assert.equal(connects[0][1].branch, "main");
  assert.deepEqual(connects[0][1].remote, { url: typed });
  assert.equal(await evaluate('git.calls.some((call) => call[0] === "push" || call[0] === "pull")'), false);

  assert.deepEqual(await evaluate("window.testErrors"), []);
  console.log("git panel browser ok");
} finally {
  await page?.close?.();
  server?.close();
  removeTemporary(temporary);
}
