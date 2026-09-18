// Browser regression check for the sharing panel: what it says at rest, and
// what it takes to change any of it.
//
// The panel answers three questions -- which kinds of access are open, how the
// link for one of them is copied, and how access is opened where it is not --
// and everything else belongs behind something. So the checks below are mostly
// about what is *not* on the screen: no form until one is asked for, never two
// at once, no rotation or revocation without a confirmation, and no second
// place that says when a link expires.
//
// The route is stubbed rather than run, because what is under test is the
// panel's two presentation modes, not the server's writes -- but the stub
// answers the way the route does, minting a fresh key for a `link` and leaving
// the key alone for a `settings`, since telling those apart is the point of
// the menu having both.
//
// The stub hands back a key on the response that mints one and on no other,
// which is what the catalogue can actually do: it keeps a link's digest, so
// `sharing_json` fills `key` and `url` only for a link this very request
// minted. A stub that kept answering with a key made a settings save look
// like it preserved the link when the real panel dropped the row.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, pause } from "../../tools/browser-driver.mjs";

process.env.TZ = "UTC";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-share-panel-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import Share from ${JSON.stringify(join(root, "web/src/components/reader/Share.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

// Two links already handed out and one role never opened, which is the state
// the panel is nearly always in.
const state = {
  reader: { key: "readkey", label: "Reviewer", until: "2027-03-17T00:00:00Z", budget: null },
  commenter: { key: "commentkey", label: "Reviewer", until: "2027-03-17T00:00:00Z", budget: null },
  editor: null,
};
window.calls = [];
const expiryOf = (until) => {
  if (until === "never") return "";
  if (!until) return null; // omitted: whatever the link already said
  return new Date(Date.now() + Number(until.replace(/[a-z]/g, "")) * 86400000).toISOString();
};
const answer = (minted) => ({
  slug: "paper", url: "/docs/paper", can_share: true,
  edit_needs_signin: true, comment_needs_signin: true,
  links: Object.fromEntries(Object.entries(state).map(([role, link]) => [role, link
    ? { key: role === minted ? link.key : "", url: role === minted ? "/docs/paper#k=" + link.key : "",
        since: "2026-01-01T00:00:00Z", until: link.until,
        label: link.label, budget: link.budget, expired: false }
    : null])),
});
window.fetch = async (path, options) => {
  const body = options?.body ? JSON.parse(options.body) : null;
  window.calls.push([path, body]);
  if (body?.revoke) state[body.revoke] = null;
  if (body?.link) {
    const was = state[body.link.role];
    state[body.link.role] = {
      key: "minted-" + window.calls.length,
      label: body.link.label ?? was?.label ?? "",
      until: expiryOf(body.link.until) ?? was?.until ?? "",
      budget: body.link.budget ?? null,
    };
  }
  if (body?.settings) {
    const was = state[body.settings.role];
    state[body.settings.role] = { ...was,
      label: body.settings.label ?? was.label,
      until: expiryOf(body.settings.until) ?? was.until,
      budget: body.settings.budget ?? null };
  }
  return { ok: true, json: async () => answer(body?.link ? body.link.role : null) };
};
Object.defineProperty(navigator, "clipboard", {
  configurable: true, value: { writeText: async (value) => { window.copied = value; } },
});

createClassComponent({ component: Share, target: document.body, props: {
  open: true, inline: true, slug: "paper", canShare: true, bundleReady: true,
} });
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 40)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };
const text = () => document.body.innerText.replace(/\\s+/g, " ").trim();
const sections = () => [...document.querySelectorAll(".share-section")];
const seen = (index) => sections()[index].innerText.replace(/\\s+/g, " ").trim();
const button = (label) => [...document.querySelectorAll("button")]
  .find((node) => (node.getAttribute("aria-label") || node.textContent).trim() === label);

window.sharePanelReady = () => Boolean(document.querySelector(".share-section"));
window.sharePanelSeen = seen;

window.sharePanelCheck = async () => {
  await flush();

  /* ------------------------------------------- what readers are served */

  // There is no publishing to approve and no version to choose: the shared
  // rendering follows the draft, so a panel with nothing wrong to report says
  // nothing at all about it and goes straight to the links.
  check(!text().includes("Published version"), "nothing calls the rendering a published version");
  check(!/up to date/i.test(text()), "a healthy shared view is not worth a line: " + text());
  check(/^Sharing Read/i.test(text()),
    "the panel opens straight on the access it hands out: " + text());

  /* ------------------------------------- a permission is what it is, not a form */

  check(sections().length === 3, "read, comment and edit");
  check(/^Read Anyone with this link can view\\. Expires Mar 1[67], 2027 The link is shown only when it is made\\. Replace it to get a new URL\\.$/.test(seen(0)),
    "a link that exists is its expiry and a row of icons that spell nothing: " + seen(0));
  check(!seen(0).includes("•••"), "nothing is behind an overflow any more: " + seen(0));
  // No copy among them: the server cannot say this link's URL again, so the
  // row offers the three things it can still do rather than a button that
  // would put an empty string on the clipboard.
  check([...sections()[0].querySelectorAll(".share-icon")].map((node) => node.getAttribute("aria-label")).join("|")
    === "Edit Read link settings|Replace Read link|Revoke Read link",
    "the verbs are flat, in the order configure, replace, revoke: "
      + [...sections()[0].querySelectorAll(".share-icon")].map((node) => node.getAttribute("aria-label")).join("|"));
  check([...sections()[0].querySelectorAll(".share-icon svg")].length === 3,
    "and each of them is drawn rather than written");
  check(seen(1).startsWith("Comment Anyone with this link can view and comment. Sign-in required."),
    "each says what the link it hands out lets somebody do: " + seen(1));
  check(seen(2) === "Edit Signed-in users with this link can edit. Sign-in required. Create link",
    "and a role with no link offers to open one rather than saying it has none: " + seen(2));
  check(!text().includes("Unlabelled"), "a link without a memo says nothing about labels");
  check(!document.querySelector(".share-section input, .share-section select"),
    "no configuration is on the screen until it is asked for");
  check(!document.querySelector(".share-section .preset-filled-primary-500"),
    "and nothing about an existing link is loud enough to be the panel's main action");

  check(!button("Copy Read link"), "a link whose URL the server cannot repeat has nothing to copy");

  /* ------------------------------------------------- opening access */

  button("Create Edit link").click(); await flush();
  check(document.querySelectorAll(".share-form").length === 1
    && sections()[2].querySelector(".share-form"),
    "creating expands that permission, and only that one");
  check(!seen(2).includes("Advanced settings") && seen(2).includes("Comments/hour"),
    "with the rate limit in the form rather than behind a disclosure: " + seen(2));
  const expiry = sections()[2].querySelector("select");
  expiry.value = "30d"; expiry.dispatchEvent(new Event("change", { bubbles: true }));
  await flush();
  button("Create link").click(); await flush();
  const minted = window.calls.at(-1)[1];
  check(minted.link?.role === "editor" && minted.link.until === "30d" && !("label" in minted.link),
    "creation mints the link that was described, and names it nothing: " + JSON.stringify(minted));
  check(!document.querySelector(".share-form")
    && sections()[2].querySelector('[aria-label="Copy Edit link"]'),
    "and it collapses to the state it is now in, holding the one URL it will "
      + "ever be handed: " + seen(2));
  // The key the panel holds is a path; what is copied is a URL.
  button("Copy Edit link").click(); await flush();
  check(window.copied === location.origin + "/docs/paper#k=" + state.editor.key,
    "copied: " + window.copied);
  return true;
};

window.shareClickLabel = (label) => { button(label).click(); };
window.shareClickWord = (says) => {
  [...document.querySelectorAll("button")].find((node) => node.textContent.trim() === says).click();
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "share-panel-check.js" } } });
  // The built shell's stylesheet carries the theme -- every colour and step of
  // spacing the panel uses is a custom property defined there.
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
    response.end('<!doctype html><html data-theme="librepaper"><head>'
      + '<link rel="stylesheet" href="/theme.css"><link rel="stylesheet" href="/style.css">'
      + '<style>body{margin:0;width:340px}</style></head>'
      + '<body><script type="module" src="/share-panel-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 24000 + Math.floor(Math.random() * 1000));
  await tab.resize(360, 900);
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("share panel component", () => tab.evaluate("Boolean(window.sharePanelReady && window.sharePanelReady())"));
  assert.equal(await tab.evaluate("window.sharePanelCheck()"), true);

  /* ------------------------------------ the rest of it is the rest of the row */

  // Editing settings is the one of the three that changes nothing about who
  // can get in, so it is the one that opens straight away.
  await tab.evaluate('window.shareClickLabel("Edit Read link settings")');
  await pause(500);
  assert.equal(await tab.evaluate("document.querySelectorAll('.share-form').length"), 1,
    "editing settings opens that permission and no other");
  await tab.evaluate(`(() => {
    const field = document.querySelector('.share-section select');
    field.value = '30d';
    field.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
  await tab.evaluate('window.shareClickWord("Save")');
  await pause(400);
  const saved = JSON.parse(await tab.evaluate("JSON.stringify(window.calls.at(-1)[1])"));
  assert.ok(saved.settings && !saved.link,
    "a change of settings does not mint a key: " + JSON.stringify(saved));
  assert.equal(saved.settings.until, "30d",
    "and carries the expiry that was chosen: " + JSON.stringify(saved));
  // The save mints nothing, so the response carries no key. A panel that read
  // a link's existence off its key collapsed the row here and offered to
  // create the link that was still sitting there.
  const after = await tab.evaluate("window.sharePanelSeen(0)");
  assert.ok(after.includes("Expires"),
    "the collapsed state then says when it now runs out: " + after);
  assert.ok(!after.includes("Create link"),
    "and the link is still a link, not an offer to make one: " + after);

  // The two that do invalidate a URL ask first.
  await tab.evaluate('window.shareClickLabel("Revoke Comment link")');
  await pause(600);
  const asked = await tab.evaluate("document.body.innerText.replace(/\\\\s+/g, ' ')");
  assert.ok(asked.includes("Revoke Comment access?"), "revoking asks first: " + asked);
  await tab.evaluate('window.shareClickWord("Revoke link")');
  await pause(500);
  assert.ok((await tab.evaluate("window.sharePanelSeen(1)")).includes("Create link"),
    "and answered, the permission is back to offering a link");

  console.log("share panel: access as a state, a flat row of icons per link, rotation and revocation confirmed");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
