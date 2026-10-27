// Browser regression check for the Git panel in both of its modes.
//
// A folder project (desktop app): the LibrePaper section that the Sync tab
// used to be -- sign in, link or publish, linked state, pull with conflicts, push
// and its refusals, unlink, and the repository form of a pull in a git folder --
// and the Repository section: no git, no repository, branch and remote, the
// change list, commit and the identity prompt, branches, pull and push with
// their refusals and the sign-in help, Sync, and the Diff button.
//
// A web project: the last push, and in the desktop app the connection to a
// repository: the connect form and its refusals, a folder, Pull and Push with
// the version name, the merge settled file by file, and the sign-in.
//
// Both modes are drawn from 260 to 600 px wide without sideways scrolling.
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
import { createLocalSync } from ${JSON.stringify(join(root, "web/src/lib/reader/local-sync.svelte.js"))};
import { createLocalGit } from ${JSON.stringify(join(root, "web/src/lib/reader/local-git.svelte.js"))};
import { createGitDiff } from ${JSON.stringify(join(root, "web/src/lib/reader/git-diff.svelte.js"))};
import { createWebGit } from ${JSON.stringify(join(root, "web/src/lib/reader/web-git.svelte.js"))};
import { createWebConnection } from ${JSON.stringify(join(root, "web/src/lib/reader/web-connection.svelte.js"))};
import { day } from ${JSON.stringify(join(root, "web/src/lib/dates.js"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

const linked = (extra = {}) => ({ signed_in: true, linked: true, site: "https://app.example/", slug: "paper", pulled: 4, changes: { here: 2, web: true }, ...extra });
const noRepo = (extra = {}) => ({ repository: false, git: { found: true, version: "2.43.0" }, remotes: [], changes: [], merging: false, identity: false, ...extra });
const repoStatus = (extra = {}) => ({
  repository: true, git: { found: true, version: "2.43.0" }, branch: "main", upstream: "origin/main", ahead: 0, behind: 0,
  remotes: [{ name: "origin", url: "https://github.com/you/paper.git" }], remote: "origin",
  changes: [{ path: "intro.tex", kind: "modified" }, { path: "chapters/outline.tex", kind: "added" }],
  merging: false, identity: true, ...extra,
});

// Each scene sets the state the stand-ins answer with and the answers a call
// gives in turn. A value with fail is thrown as the desktop app's refusal.
// Signing in is the desktop app's code, the page's approval of it, and the
// finish, which refuses while the code is pending for as many tries as the
// scene asks. A scene with session false is a page with no session on the
// server: the code is then approved in another tab, and hold is the promise
// that the wait between two asks of the finish stays in until the scene
// releases it.
const scene = () => window.scene;
const record = (...entry) => scene().calls.push(entry);
const refusal = (failure) => Object.assign(new Error(failure.message || failure.body?.error || "The desktop app refused the request."), { name: "Refused", status: failure.status, body: failure.body });
const settle = (value) => { if (value?.fail) throw refusal(value.fail); return value; };

const syncApi = {
  syncStatus: async () => { record("syncStatus"); return structuredClone(scene().status); },
  syncLogin: async () => {
    record("syncLogin");
    return { authorization_id: "auth-1", user_code: "ABCD-EFGH", ...(scene().verification ? { verification_url: scene().verification } : {}) };
  },
  syncLoginComplete: async (id, authorizationId) => {
    record("syncLoginComplete", authorizationId);
    if (scene().pendingTries > 0) {
      scene().pendingTries -= 1;
      throw refusal({ status: 400, body: { error: "sign in and approve the code, then retry" } });
    }
    if (scene().expired) throw refusal({ status: 400, body: { error: "that sign-in expired" } });
    scene().status = scene().afterLogin;
    return structuredClone(scene().status);
  },
  syncLink: async (id, url) => { record("syncLink", url); scene().status = scene().afterLink; },
  syncPublish: async (id, title) => { record("syncPublish", title); scene().status = scene().afterLink; },
  syncUnlink: async () => { record("syncUnlink"); scene().status = { signed_in: true, linked: false, git: Boolean(scene().status.git) }; },
  syncPull: async (id, choices) => { record("syncPull", choices); return settle(scene().pulls.shift()); },
  syncPush: async (id, message) => { record("syncPush", message); return settle(scene().pushes.shift()); },
};

const gitApi = {
  gitStatus: async () => { record("gitStatus"); return structuredClone(scene().repo); },
  gitInit: async () => { record("gitInit"); scene().repo = scene().afterInit ?? scene().repo; },
  gitDiff: async (id, path) => { record("gitDiff", path); return settle(scene().diff ?? { head: "old\\n", work: "new\\n", binary: false }); },
  gitCommit: async (id, message, paths) => {
    record("gitCommit", message, paths);
    const answer = settle(scene().commits.shift() ?? { commit: "1a2b3c4d5e6f" });
    if (scene().afterCommit) scene().repo = scene().afterCommit;
    return answer;
  },
  gitIdentity: async (id, name, email) => { record("gitIdentity", name, email); settle(scene().identityAnswer); },
  gitBranches: async () => { record("gitBranches"); return structuredClone(scene().branches); },
  gitSwitch: async (id, branch) => {
    record("gitSwitch", branch);
    settle(scene().switchAnswer);
    scene().repo = { ...scene().repo, branch };
    scene().branches = { ...scene().branches, current: branch };
  },
  gitBranch: async (id, branch) => {
    record("gitBranch", branch);
    scene().repo = { ...scene().repo, branch };
    scene().branches = { current: branch, branches: [...scene().branches.branches, branch] };
  },
  gitRemote: async (id, name, url) => { record("gitRemote", name, url); scene().repo = { ...scene().repo, remotes: [{ name, url }], remote: name }; },
  gitFetch: async () => { record("gitFetch"); return settle(scene().fetched ?? { ahead: 0, behind: 0 }); },
  gitPull: async () => { record("gitPull"); return settle(scene().gitPulls.shift() ?? { status: "up-to-date", conflicts: [] }); },
  gitPush: async () => { record("gitPush"); return settle(scene().gitPushes.shift()); },
  gitSync: async (id, message) => { record("gitSync", message); return settle(scene().syncs.shift()); },
};

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
  calls: [], saves: 0, pulls: [], pushes: [], gitPulls: [], gitPushes: [], commits: [], syncs: [],
  status: { signed_in: false, linked: false }, connection: { connected: false, signed_in: true }, repo: noRepo(), branches: { current: "main", branches: ["main"] },
  pages: [{ cursor: null, next: null, labels: [] }],
});

const mountFolder = async (next = {}, props = {}) => {
  destroy();
  window.scene = { ...defaults(), ...next };
  const sync = createLocalSync({
    id: "f1", api: syncApi,
    approve: async (code) => { record("approve", code); },
    signedIn: async () => scene().session !== false,
    open: (url) => { record("open", url); },
    wait: () => scene().hold ?? Promise.resolve(),
    flush: async () => { scene().saves += 1; return scene().saved !== false; },
  });
  const git = createLocalGit({ id: "f1", api: gitApi, flush: async () => { scene().saves += 1; return scene().saved !== false; } });
  const differ = createGitDiff({ id: "f1", api: gitApi });
  component = createClassComponent({ component: Git, target: document.getElementById("panel"), props: {
    local: true, open: true, sync, git, differ, defaultTitle: "thesis",
    ondiff: (change) => { record("ondiff", change.path); void differ.show(change.path, change.kind); },
    ...props,
  } });
  await tick();
};

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
    local: false, open: true, web: createWebGit({ api: webApi }), slug: "paper", linkKey: "",
    desktop: false, connection, onfolder: async () => {}, ...props,
  } });
  await tick();
};

const today = () => day(new Date());
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
window.ready = true;
`;

const checks = `
const check = (condition, message) => { if (!condition) throw new Error(message); };
const squash = (node) => node.innerText.replace(/\\s+/g, " ").trim();
const lp = () => document.querySelector('[aria-labelledby="git-librepaper-title"]');
const repo = () => document.querySelector('[aria-labelledby="git-repository-title"]');
const base = () => lp() ?? document.getElementById("panel");
const text = (where = base()) => squash(where);
const panel = () => document.querySelector(".git-panel");
const button = (label, where = base()) => [...where.querySelectorAll("button")].find((node) => node.textContent.trim() === label) ?? null;
const press = async (label, where = base()) => { const node = button(label, where); check(node, "missing button " + label + ": " + text(where)); node.click(); await flush(); };
const type = async (node, value) => { check(node, "missing field"); node.value = value; node.dispatchEvent(new Event("input", { bubbles: true })); await flush(); };
const pick = async (node, value) => { check(node, "missing select"); node.value = value; node.dispatchEvent(new Event("change", { bubbles: true })); await flush(); };
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
const pressDialog = async (label) => {
  const node = [...document.querySelectorAll('[role="dialog"] button')].find((entry) => entry.textContent.trim() === label);
  check(node, "missing dialog button " + label);
  node.click();
  await flush();
};
const confirmFor = (says) => [...document.querySelectorAll('[role="dialog"] button')].some((node) => node.textContent.trim() === says);

window.gitPanelCheck = async () => {
  /* ======================================================== FOLDER MODE */
  /* ------------------------------------------ the LibrePaper section */
  /* ------------------------------------------------------ signed out */

  await mountFolder({ status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false } });
  await flush();
  check(panel().getAttribute("aria-label") === "Git", "the panel is called Git");
  check(text().includes("Sign in to keep this folder"), "a signed-out folder asks for a sign-in: " + text());
  check(!button("Pull") && !button("Link"), "and offers nothing else");
  await press("Sign in");
  check(calls("syncLogin").length === 1 && calls("approve")[0][1] === "ABCD-EFGH", "Sign in gets a code and approves it here: " + JSON.stringify(window.scene.calls));
  check(calls("syncLoginComplete")[0][1] === "auth-1", "and finishes with the authorization");
  check(text().includes("Link to a project"), "and the panel moves on to linking: " + text());

  // The finish refuses while the approval is still on its way, and the page asks again.
  await mountFolder({ status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false }, pendingTries: 2 });
  await flush();
  await press("Sign in");
  check(calls("syncLoginComplete").length === 3 && text().includes("Link to a project"), "a pending code is tried again: " + text());

  // An expired sign-in says so and leaves the Sign in button.
  await mountFolder({ status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false }, expired: true });
  await flush();
  await press("Sign in");
  check(text().includes("that sign-in expired") && button("Sign in"), "an expired sign-in goes back to the button: " + text());

  /* ----------------------------- no session on the server: another tab */

  // The page cannot approve the code, so it shows it, opens the server's page,
  // and waits until the code is approved there.
  const verification = "https://app.example/auth/device?code=ABCD-EFGH";
  let release;
  await mountFolder({
    status: { signed_in: false, linked: false }, afterLogin: { signed_in: true, linked: false },
    session: false, verification, hold: new Promise((resolve) => { release = resolve; }),
  });
  await flush();
  await press("Sign in");
  check(text().includes("Waiting for the code to be approved in the other tab"), "a page with no session waits for the other tab: " + text());
  check(lp().querySelector(".sync-code")?.textContent.trim() === "ABCD-EFGH", "and shows the code: " + text());
  check(calls("approve").length === 0, "it never approves the code itself");
  check(calls("open").length === 1 && calls("open")[0][1] === verification, "it opens the verification page the desktop app named: " + JSON.stringify(calls("open")));
  const again = [...lp().querySelectorAll("a")].find((node) => node.getAttribute("href") === verification);
  check(again && again.target === "_blank" && again.rel.includes("noopener"), "and offers the page as a link in case the tab was blocked");
  check(button("Cancel") && !button("Sign in") && !button("Cancel").disabled, "Cancel is there and Sign in is not: " + text());
  check(calls("syncLoginComplete").length === 0, "the finish has not been asked yet");
  await widths("while waiting for the other tab");
  release();
  await flush();
  check(calls("syncLoginComplete").length === 1 && text().includes("Link to a project") && !text().includes("Waiting for the code"), "once the code is approved the panel moves on: " + text());

  // Cancel goes back to the Sign in button and asks the finish for nothing.
  await mountFolder({
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

  await mountFolder({ status: { signed_in: true, linked: false }, afterLink: linked() });
  await flush();
  check(button("Link").disabled, "Link waits for an address");
  check(field("Or publish").value === "thesis", "the title offered is the folder's name");
  check(!button("Publish").disabled, "so Publish is ready");
  await type(field("Link to a project"), "  https://app.example/docs/paper  ");
  await press("Link");
  check(calls("syncLink")[0][1].trim() === "https://app.example/docs/paper", "Link sends the address: " + JSON.stringify(calls("syncLink")));
  check(text().includes("paper"), "and the linked project shows: " + text());

  await mountFolder({ status: { signed_in: true, linked: false }, afterLink: linked({ changes: { here: 0, web: false } }) });
  await flush();
  await type(field("Or publish"), "A different title");
  await press("Publish");
  check(calls("syncPublish")[0][1] === "A different title", "Publish sends the title: " + JSON.stringify(calls("syncPublish")));

  /* --------------------------------------------------------------- linked */

  await mountFolder({ status: linked() });
  await flush();
  const link = lp().querySelector(".sync-project a");
  check(link && link.getAttribute("href") === "https://app.example/docs/paper" && link.textContent.trim() === "paper", "the project is a link to its page: " + (link && link.getAttribute("href")));
  check(text().includes("2 changes here") && text().includes("The web project changed"), "the state is said in two lines: " + text());
  check(button("Pull") && button("Push") && button("Unlink"), "Pull, Push and Unlink are there");
  check(field("Message").value === today(), "the message starts as today's date: " + field("Message").value);
  check(!text().includes("This folder is a git repository"), "a plain folder does not talk about git here");

  await mountFolder({ status: linked({ changes: { here: 0, web: false } }) });
  await flush();
  check(text().includes("No changes here") && text().includes("Up to date") && !text().includes("The web project changed"), "a quiet folder says so: " + text());
  await mountFolder({ status: linked({ changes: { here: 1, web: false } }) });
  await flush();
  check(text().includes("1 change here"), "one change is not plural: " + text());

  /* ---------------------------------------------------- a git repository */

  // The desktop no longer refuses Pull and Push in a git folder: the section
  // stays whole, the flag only picks the wording, and a pull answers in its
  // repository form -- the versions taken as commits, then how git merged them.
  const version = (n) => ({ sha: "sha" + n, label: "v" + n, by: "anne", commit: "c" + n });
  await mountFolder({
    status: linked({ git: true }),
    pulls: [
      { repository: true, versions: [version(1), version(2), version(3)], merge: "fast-forward", trimmed: false },
      { repository: true, versions: [], merge: "already", trimmed: false },
      { repository: true, versions: [version(4)], merge: "merged", trimmed: true },
      { repository: true, versions: [version(5)], merge: "skipped", trimmed: false },
      { repository: true, versions: [version(6)], merge: { conflicts: ["intro.tex", "chapters/refs.bib"] }, trimmed: false },
      { repository: true, versions: [], merge: { dirty: ["intro.tex"] }, trimmed: false },
    ],
    pushes: [{ label: { sha: "abc", sequence: 5 }, files: 2 }],
  });
  await flush();
  check(text().includes("This folder is a git repository, so a pull takes each web version in as a commit"), "a git folder says how a pull works: " + text());
  check(button("Pull") && button("Push") && button("Unlink") && field("Message") && lp().querySelector(".sync-project"), "and keeps Pull, Push, Unlink and the project");
  check(!text().includes("Sync it with git in a terminal"), "the notice that hid them is gone");
  await press("Pull");
  check(text().includes("3 versions fetched as commits.") && text().includes("Fast-forwarded."), "a pull says the versions and the merge: " + text());
  await press("Pull");
  check(text().includes("No new versions on the web.") && text().includes("Already merged."), "a pull with nothing new says so: " + text());
  await press("Pull");
  check(text().includes("1 version fetched as commits.") && text().includes("Merged.") && text().includes("was trimmed"), "a merge, and a trimmed history: " + text());
  await press("Pull");
  check(text().includes("Not merged."), "a skipped merge says so: " + text());
  await press("Pull");
  check(text().includes("Merge conflicts in intro.tex, chapters/refs.bib. Resolve them in the editor, then Commit in the Repository section."), "conflicts name the files and where to go: " + text());
  check(!button("Keep mine") && !button("Take theirs"), "there is no side to take in a repository");
  await press("Pull");
  check(text().includes("Commit your changes first: intro.tex"), "uncommitted files stop the merge and are named: " + text());
  check(!text().includes("Merge conflicts"), "the earlier result is gone");
  await press("Push");
  check(text().includes("Pushed 2 files."), "a push works as before: " + text());

  /* ----------------------------------------------------------------- pull */

  await mountFolder({
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

  await mountFolder({ status: linked({ changes: { here: 0, web: false } }), pulls: [{ written: [], trashed: [], kept: [], conflicts: [] }] });
  await flush();
  await press("Pull");
  check(text().includes("Nothing new on the web."), "a pull with nothing says so: " + text());
  await mountFolder({ status: linked(), pulls: [{ written: ["a.tex"], trashed: [], conflicts: [], trimmed: true }] });
  await flush();
  await press("Pull");
  check(text().includes("was trimmed"), "a trimmed history is said: " + text());
  await mountFolder({ status: linked(), pulls: [{ fail: { status: 401, body: { error: "sign in" }, message: "sign in" } }] });
  await flush();
  await press("Pull");
  check(text().includes("Sign in to the web project to continue.") && button("Sign in"), "a lost sign-in goes back to the Sign in button: " + text());

  /* ----------------------------------------------------------------- push */

  await mountFolder({
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

  await mountFolder({ status: linked(), saved: false });
  await flush();
  await press("Push");
  check(calls("syncPush").length === 0 && text().includes("could not be saved"), "a folder that cannot be saved stops the push: " + text());

  /* --------------------------------------------------------------- unlink */

  await mountFolder({ status: linked() });
  await flush();
  await press("Unlink");
  check(calls("syncUnlink").length === 1 && text().includes("Link to a project"), "Unlink goes back to linking: " + text());

  /* ------------------------------------------- the Repository section */
  /* ------------------------------------------------------- no git, no repository */

  await mountFolder({ repo: noRepo({ git: { found: false } }) });
  await flush();
  check(text(repo()).includes("Git was not found on this computer") && text(repo()).includes("2.38"), "without git the section says so: " + text(repo()));
  const setup = [...repo().querySelectorAll("a")].find((node) => node.getAttribute("href") === "/docs/git.html");
  check(setup && setup.target === "_blank", "and names the docs");
  check(!button("Initialize", repo()) && !button("Commit", repo()), "and offers nothing to run");
  await mountFolder({ repo: noRepo({ git: { found: true, version: "2.30.0", supported: false } }) });
  await flush();
  check(text(repo()).includes("Git 2.30.0 is installed, and LibrePaper needs Git 2.38 or later."), "an old git is named: " + text(repo()));

  await mountFolder({ repo: noRepo(), afterInit: repoStatus({ changes: [], remotes: [], upstream: undefined }) });
  await flush();
  check(text(repo()).includes("This folder is not a git repository."), "a folder without a repository says so: " + text(repo()));
  check(!repo().querySelector("select"), "and no branch yet");
  await press("Initialize", repo());
  check(calls("gitInit").length === 1, "Initialize asks the desktop app");
  check(repo().querySelector("select")?.value === "main" && text(repo()).includes("Made this folder a git repository"), "and the branch appears: " + text(repo()));

  /* ------------------------------------------------------- a repository at rest */

  await mountFolder({ repo: repoStatus(), branches: { current: "main", branches: ["main", "draft"] } });
  await flush();
  const select = () => repo().querySelector('select[aria-label="Branch"]');
  check(select().value === "main" && [...select().options].map((node) => node.value).join("|") === "main|draft", "the branch menu lists the branches: " + [...select().options].map((node) => node.value));
  check(text(repo()).includes("Up to date with origin/main.") && text(repo()).includes("origin https://github.com/you/paper.git"), "the upstream and the remote are said: " + text(repo()));
  check(text(repo()).includes("Changes (2)"), "the changes are counted: " + text(repo()));
  const changeRows = () => [...repo().querySelectorAll(".git-change")];
  check(changeRows().length === 2, "each change has a row");
  check(squash(changeRows()[0]).includes("intro.tex") && squash(changeRows()[0]).includes("M"), "a modified file is marked M: " + squash(changeRows()[0]));
  check(squash(changeRows()[1]).includes("outline.tex") && squash(changeRows()[1]).includes("chapters") && squash(changeRows()[1]).includes("A"), "an added one A, with its folder: " + squash(changeRows()[1]));
  check(changeRows().every((row) => row.querySelector("input[type=checkbox]")?.checked && button("Diff", row)), "each is included and has a Diff button");
  check(button("Commit", repo()).disabled, "Commit waits for a message");
  check(button("Pull", repo()) && button("Push", repo()) && button("Fetch", repo()) && button("Sync", repo()), "Pull, Push, Fetch and Sync are there");
  check(!button("Pull", repo()).disabled, "and ready");
  check(text(repo()).includes("Commits, pulls and pushes."), "Sync says what it does");

  await mountFolder({ repo: repoStatus({ ahead: 2, behind: 1 }) });
  await flush();
  check(text(repo()).includes("2 ahead, 1 behind origin/main."), "ahead and behind are counted: " + text(repo()));
  await mountFolder({ repo: repoStatus({ upstream: undefined, ahead: undefined, behind: undefined }) });
  await flush();
  check(text(repo()).includes("No upstream branch yet. The first push sets it."), "a branch with no upstream says the first push sets it: " + text(repo()));
  await mountFolder({ repo: repoStatus({ remotes: [], upstream: undefined }) });
  await flush();
  check(text(repo()).includes("No remote yet. Set one to pull and push.") && text(repo()).includes("No remote"), "no remote is said: " + text(repo()));
  check(button("Pull", repo()).disabled && button("Push", repo()).disabled && button("Sync", repo()).disabled && !button("Set remote", repo()).disabled, "and there is nothing to pull from or push to");
  check(changeRows().length === 2, "the changes are still listed");

  /* ------------------------------------------------------------------ commit */

  await mountFolder({ repo: repoStatus(), afterCommit: repoStatus({ changes: [], ahead: 1 }) });
  await flush();
  await type(field("Message", repo()), "Fix the abstract");
  check(!button("Commit", repo()).disabled, "a message enables Commit");
  await press("Commit", repo());
  check(window.scene.saves === 1, "the folder is saved before the commit");
  check(calls("gitCommit").length === 1 && calls("gitCommit")[0][1] === "Fix the abstract" && calls("gitCommit")[0][2] === undefined, "Commit sends the message and, by default, every file: " + JSON.stringify(calls("gitCommit")));
  check(text(repo()).includes("Committed 1a2b3c4."), "and says what it made: " + text(repo()));
  check(text(repo()).includes("No changes.") && field("Message", repo()).value === "", "the list empties and the message goes: " + text(repo()));

  // A file can be left out.
  await mountFolder({ repo: repoStatus() });
  await flush();
  const boxes = () => [...repo().querySelectorAll(".git-change input[type=checkbox]")];
  boxes()[1].checked = false;
  boxes()[1].dispatchEvent(new Event("change", { bubbles: true }));
  await type(field("Message", repo()), "Only the intro");
  await press("Commit", repo());
  check(JSON.stringify(calls("gitCommit")[0][2]) === JSON.stringify(["intro.tex"]), "a file left out is not committed: " + JSON.stringify(calls("gitCommit")));

  // A renamed file that is committed sends its old name too, so the deletion goes with it.
  await mountFolder({ repo: repoStatus({ changes: [{ path: "new.tex", kind: "renamed", from: "old.tex" }, { path: "intro.tex", kind: "modified" }] }) });
  await flush();
  boxes()[1].checked = false;
  boxes()[1].dispatchEvent(new Event("change", { bubbles: true }));
  await type(field("Message", repo()), "Rename the chapter");
  await press("Commit", repo());
  check(JSON.stringify(calls("gitCommit")[0][2]) === JSON.stringify(["new.tex", "old.tex"]), "a renamed file in a partial commit sends its old name: " + JSON.stringify(calls("gitCommit")));

  // A file written again at the old name of a rename has a box of its own. Left
  // out, it is not sent with the rename. With both boxes ticked, every file goes.
  await mountFolder({ repo: repoStatus({ changes: [{ path: "new.tex", kind: "renamed", from: "old.tex" }, { path: "old.tex", kind: "added" }] }) });
  await flush();
  boxes()[1].checked = false;
  boxes()[1].dispatchEvent(new Event("change", { bubbles: true }));
  await type(field("Message", repo()), "Rename the chapter");
  await press("Commit", repo());
  check(JSON.stringify(calls("gitCommit")[0][2]) === JSON.stringify(["new.tex"]), "a file written again at the old name is left out with its box: " + JSON.stringify(calls("gitCommit")));
  await type(field("Message", repo()), "Rename and keep the new file");
  await press("Commit", repo());
  check(calls("gitCommit").length === 2 && calls("gitCommit")[1][2] === undefined, "with both ticked, the commit takes every file, old name included: " + JSON.stringify(calls("gitCommit")));

  // The first commit in a repository with no identity asks for one.
  await mountFolder({
    repo: repoStatus({ identity: false }),
    commits: [{ fail: { status: 409, body: { reason: "identity" } } }, { commit: "abcdef0123456" }],
  });
  await flush();
  await type(field("Message", repo()), "First commit");
  await press("Commit", repo());
  check(text(repo()).includes("Git needs a name and an email for your commits."), "a missing identity is asked for: " + text(repo()));
  check(!text(repo()).includes("refused"), "and is not an error");
  check(button("Save and commit", repo()).disabled, "Save waits for both");
  await type(field("Name", repo()), "Anne Example");
  await type(field("Email", repo()), "anne@example.org");
  await press("Save and commit", repo());
  check(calls("gitIdentity").length === 1 && calls("gitIdentity")[0][1] === "Anne Example" && calls("gitIdentity")[0][2] === "anne@example.org", "the identity is saved: " + JSON.stringify(calls("gitIdentity")));
  check(calls("gitCommit").length === 2 && calls("gitCommit")[1][1] === "First commit", "and the commit is made again with the same message");
  check(text(repo()).includes("Committed abcdef0.") && !text(repo()).includes("Git needs a name"), "the prompt is gone: " + text(repo()));

  await mountFolder({ repo: repoStatus({ identity: false }), commits: [{ fail: { status: 409, body: { reason: "identity" } } }] });
  await flush();
  await type(field("Message", repo()), "First commit");
  await press("Commit", repo());
  await press("Cancel", repo());
  check(!text(repo()).includes("Git needs a name") && field("Message", repo()), "Cancel puts the prompt away: " + text(repo()));

  /* ---------------------------------------------------------------- branches */

  await mountFolder({ repo: repoStatus(), branches: { current: "main", branches: ["main", "draft"] } });
  await flush();
  await pick(select(), "draft");
  check(calls("gitSwitch").length === 1 && calls("gitSwitch")[0][1] === "draft", "choosing a branch switches to it: " + JSON.stringify(window.scene.calls));
  check(window.scene.saves === 1, "after the folder is saved");
  check(select().value === "draft" && text(repo()).includes("Switched to draft."), "and the menu follows: " + text(repo()));

  await mountFolder({ repo: repoStatus(), branches: { current: "main", branches: ["main"] } });
  await flush();
  await press("New branch", repo());
  check(button("Create", repo()).disabled && repo().querySelector('input[aria-label="Name of the new branch"]'), "New branch asks for a name");
  await type(repo().querySelector('input[aria-label="Name of the new branch"]'), "review");
  await press("Create", repo());
  check(calls("gitBranch").length === 1 && calls("gitBranch")[0][1] === "review", "Create makes the branch: " + JSON.stringify(window.scene.calls));
  check(select().value === "review" && !repo().querySelector('input[aria-label="Name of the new branch"]'), "and the form closes on it: " + text(repo()));

  /* ------------------------------------------------------------------ remote */

  await mountFolder({ repo: repoStatus() });
  await flush();
  await press("Set remote", repo());
  check(field("Address of the remote", repo()).value === "https://github.com/you/paper.git", "Set remote starts from the current address");
  await type(field("Address of the remote", repo()), "https://you:secret@github.com/you/paper.git");
  await press("Save", repo());
  check(calls("gitRemote").length === 0 && text(repo()).includes("password"), "an address with a password is not sent: " + text(repo()));
  await type(field("Address of the remote", repo()), "git@github.com:you/paper.git");
  await press("Save", repo());
  check(calls("gitRemote").length === 1 && calls("gitRemote")[0][1] === "origin" && calls("gitRemote")[0][2] === "git@github.com:you/paper.git", "a good one is sent to the same remote: " + JSON.stringify(calls("gitRemote")));
  check(text(repo()).includes("origin git@github.com:you/paper.git") && !field("Address of the remote", repo()), "and shown: " + text(repo()));

  /* ------------------------------------------------------------------- fetch */

  await mountFolder({ repo: repoStatus(), fetched: { ahead: 2, behind: 1 } });
  await flush();
  await press("Fetch", repo());
  check(text(repo()).includes("Fetched. 2 ahead, 1 behind."), "a fetch says how far the branch is: " + text(repo()));

  /* -------------------------------------------------------------------- pull */

  await mountFolder({
    repo: repoStatus(),
    gitPulls: [
      { status: "merged", conflicts: [] },
      { status: "conflicts", conflicts: ["intro.tex", "chapters/refs.bib"] },
      { fail: { status: 409, body: { reason: "dirty", paths: ["notes.tex"] } } },
    ],
  });
  await flush();
  await press("Pull", repo());
  check(window.scene.saves === 1 && text(repo()).includes("Pulled and merged."), "a pull saves the folder, then says how it went: " + text(repo()));
  await press("Pull", repo());
  check(text(repo()).includes("2 files in conflict. Resolve them in the editor, then Commit."), "a conflict says to resolve it in the editor: " + text(repo()));
  check([...repo().querySelectorAll(".git-list li")].map((node) => node.textContent).join("|") === "intro.tex|chapters/refs.bib", "and lists the files");
  check(!text(repo()).includes("Pulled and merged."), "the earlier result is gone");
  await press("Pull", repo());
  check(text(repo()).includes("not committed") && text(repo()).includes("notes.tex") && text(repo()).includes("Commit them, then pull again."), "uncommitted files are listed and stop the pull: " + text(repo()));

  // A merge in progress: Commit finishes it, every file goes in, and the message is git's.
  await mountFolder({ repo: repoStatus({ merging: true, changes: [{ path: "intro.tex", kind: "conflicted" }, { path: "refs.bib", kind: "modified" }] }) });
  await flush();
  check(text(repo()).includes("A merge is in progress. Resolve any conflicts in the editor, then Commit."), "a merge in progress is said: " + text(repo()));
  check(!repo().querySelector(".git-change input[type=checkbox]"), "no file can be left out of a merge");
  check(squash(repo().querySelector(".git-change")).includes("U"), "a conflicted file is marked U");
  check(field("Message", repo()).value === "Merge origin/main" && !button("Commit", repo()).disabled, "the message starts as git's own words: " + field("Message", repo()).value);
  await press("Commit", repo());
  check(calls("gitCommit")[0][1] === "Merge origin/main" && calls("gitCommit")[0][2] === undefined, "Commit completes the merge with every file: " + JSON.stringify(calls("gitCommit")));

  /* -------------------------------------------------------------------- push */

  await mountFolder({
    repo: repoStatus(),
    gitPushes: [
      undefined,
      { fail: { status: 409, body: { reason: "behind" } } },
      { fail: { status: 409, body: { reason: "network", detail: "Could not resolve host: github.com" } } },
      { fail: { status: 409, body: { reason: "auth", detail: "remote: Invalid username or password." } } },
    ],
  });
  await flush();
  await press("Push", repo());
  check(window.scene.saves === 0, "a push does not wait for the folder");
  check(text(repo()).includes("Pushed."), "a push says so: " + text(repo()));
  await press("Push", repo());
  check(text(repo()).includes("The remote has commits you do not have. Pull first.") && !text(repo()).includes("Pushed."), "a remote that moved says to pull first: " + text(repo()));
  await press("Push", repo());
  check(text(repo()).includes("Git could not reach the remote") && text(repo()).includes("Could not resolve host: github.com"), "a lost connection is said with git's words: " + text(repo()));
  await press("Push", repo());
  check(text(repo()).includes("Git could not sign in to github.com."), "a failed sign-in shows the sign-in help: " + text(repo()));
  check(text(repo()).includes("Install the GitHub CLI.") && text(repo()).includes("gh auth login") && text(repo()).includes("gh auth setup-git"), "with the steps for github.com: " + text(repo()));
  check(repo().querySelector('button[aria-label="Copy gh auth login"]'), "each command can be copied");
  check(text(repo()).includes("remote: Invalid username or password."), "git's own words are kept");
  const cli = [...repo().querySelectorAll(".git-steps a")].find((node) => node.getAttribute("href") === "https://cli.github.com");
  check(cli && cli.target === "_blank" && cli.rel.includes("noopener"), "a step can be a link");
  await widths("with the sign-in help");
  await press("Try again", repo());
  check(calls("gitPush").length === 5 && text(repo()).includes("Pushed.") && !text(repo()).includes("Git could not sign in"), "Try again pushes again, and the help goes: " + text(repo()));

  // The help is for the host the remote is on, and for how it is reached.
  await mountFolder({
    repo: repoStatus({ remotes: [{ name: "origin", url: "git@gitlab.com:group/paper.git" }], remote: "origin" }),
    gitPushes: [{ fail: { status: 409, body: { reason: "auth", detail: "Permission denied (publickey)." } } }],
  });
  await flush();
  await press("Push", repo());
  check(text(repo()).includes("Git could not sign in to gitlab.com over SSH.") && text(repo()).includes("ssh -T git@gitlab.com"), "an SSH remote gets the SSH steps: " + text(repo()));
  await press("Dismiss", repo());
  check(!text(repo()).includes("Git could not sign in"), "Dismiss puts the help away");

  /* -------------------------------------------------------------------- sync */

  await mountFolder({
    repo: repoStatus(),
    syncs: [
      { steps: [{ step: "commit", commit: "1a2b3c4d" }, { step: "pull", status: "merged", conflicts: [] }, { step: "push" }] },
      { steps: [{ step: "commit", status: "nothing" }, { step: "pull", status: "up-to-date", conflicts: [] }, { step: "push", reason: "behind" }] },
    ],
  });
  await flush();
  await press("Sync", repo());
  check(calls("gitSync").length === 1 && calls("gitSync")[0][1] === "Edits from LibrePaper", "Sync commits with its own message: " + JSON.stringify(calls("gitSync")));
  check(window.scene.saves === 1, "after saving the folder");
  check(text(repo()).includes("Committed 1a2b3c4.") && text(repo()).includes("Pulled and merged.") && text(repo()).includes("Pushed."), "every step is shown: " + text(repo()));
  await press("Sync", repo());
  check(text(repo()).includes("Nothing to commit.") && text(repo()).includes("Stopped at push") && text(repo()).includes("Pull first."), "a Sync stops at the step that failed and says which: " + text(repo()));
  check(!text(repo()).includes("Pulled and merged."), "the earlier steps are gone");

  /* -------------------------------------------------------------------- diff */

  await mountFolder({ repo: repoStatus() });
  await flush();
  const diffButton = () => button("Diff", changeRows()[0]);
  await press("Diff", changeRows()[0]);
  check(calls("ondiff").length === 1 && calls("ondiff")[0][1] === "intro.tex", "Diff names the file to the reader: " + JSON.stringify(calls("ondiff")));
  check(calls("gitDiff").length === 1 && calls("gitDiff")[0][1] === "intro.tex", "and the differ reads it");
  check(diffButton().getAttribute("aria-current") === "true" && !button("Diff", changeRows()[1]).getAttribute("aria-current"), "the file on screen is marked");

  /* ---------------------------------------------------------- narrow panel */

  await mountFolder({
    status: linked({ slug: "a-project-with-a-name-long-enough-to-need-cutting-short" }),
    repo: repoStatus({
      branch: "feature/a-branch-with-a-name-that-keeps-going-and-going-and-going",
      upstream: "origin/feature/a-branch-with-a-name-that-keeps-going-and-going-and-going", ahead: 12, behind: 345,
      remotes: [{ name: "origin", url: "https://example-forge.example/organisation/a-very-long-repository-name-that-keeps-going.git" }], remote: "origin",
      changes: [
        { path: "chapters/a-rather-long-folder-name/rendering-notes-for-the-appendix.qmd", kind: "modified" },
        { path: "a-file-with-a-very-long-name-and-no-folder-at-all-to-cut-it-short.tex", kind: "renamed", from: "old.tex" },
      ],
    }),
    branches: { current: "main", branches: ["main", "feature/a-branch-with-a-name-that-keeps-going-and-going-and-going"] },
    pulls: [{ written: ["a.tex"], trashed: [], conflicts: ["chapters/a-rather-long-folder-name/rendering-notes-for-the-appendix.qmd"] }],
    pushes: [{ fail: { status: 409, body: { reason: "changed", paths: ["chapters/a-rather-long-folder-name/rendering-notes-for-the-appendix.qmd"] } } }],
    gitPulls: [{ status: "conflicts", conflicts: ["chapters/a-rather-long-folder-name/rendering-notes-for-the-appendix.qmd"] }],
    gitPushes: [{ fail: { status: 409, body: { reason: "auth", detail: "fatal: Authentication failed for https://example-forge.example/organisation/a-very-long-repository-name-that-keeps-going.git" } } }],
  });
  await flush();
  await press("Pull", lp());
  await press("Push", lp());
  await press("Pull", repo());
  await press("Push", repo());
  await widths("in a folder project");
  await press("Set remote", repo());
  await widths("with the remote form open");
  await press("Cancel", repo());
  await press("New branch", repo());
  await widths("with the branch form open");

  /* ============================================================ WEB MODE */

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
  check(!panel().querySelector("pre") && !button("Connect") && !button("Use a folder on this computer…"), "the browser shows no commands and no connect form");
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

  // In the desktop app and not connected, the panel connects the project: the address is checked and trimmed, and a folder can be kept instead.
  const connectedStatus = { connected: true, signed_in: true, url: "https://github.com/you/paper.git", branch: "main", merging: null };
  let folders = 0;
  await mountWeb({ connection: { connected: false, signed_in: true }, afterConnect: connectedStatus }, { desktop: true, onfolder: async () => { folders += 1; } });
  await flush();
  check(headings() === "Repository|Connect a repository", "the desktop app adds the connect section: " + headings());
  check(text().includes("Connect this project to a git repository. Git uses your own sign-in."), "and says what it does: " + text());
  check(field("Repository address")?.placeholder === "https://github.com/owner/repo.git", "an address field with an example");
  check(text().includes("To edit in a clone you already have, or in a new empty folder."), "the folder choice says what it takes");
  await widths("with the connect form");

  await type(field("Repository address"), "https://user:secret@github.com/you/paper.git");
  await press("Connect");
  check(alert().includes("That address contains a password") && calls("connectRepository").length === 0, "an address with a password is refused here, and not sent: " + alert());

  await press("Use a folder on this computer…");
  check(folders === 1, "the folder button calls onfolder");

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
  console.log("git panel: folder mode (sign-in, link, pull, push, repository section, commit, branches, remote, sync, diff) and web mode (last push, connection to a repository), narrow widths");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
