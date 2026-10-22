// Browser regression check for the sharing panel: links and agent tokens as
// two lists of two-line rows, the More menu, the confirmations, the one-time
// token screen and the collapsed expired section.
//
// The route is stubbed rather than run, because what is under test is the
// panel, not the server's writes. The stub follows the share contract: every
// row the document keeps comes back in `links` or `tokens`, live or expired,
// and create and replace add a `created` object. One link has no URL on
// purpose: it stands for a link made before keys were kept.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync, existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, pause, removeTemporary } from "../helpers/browser-driver.mjs";
import { assertFreshDist } from "../helpers/fresh-dist.mjs";

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

let counter = 0;
const idOf = () => String(++counter).padStart(16, "0");
const week = () => new Date(Date.now() + 7 * 86400000).toISOString();
const state = {
  links: [
    { id: "a000000000000001", role: "reader", name: "", key: "", url: "", since: "2026-01-01T00:00:00Z", until: "2027-03-17T00:00:00Z", expired: false, budget: null },
    { id: "a000000000000002", role: "commenter", name: "Reviewer", key: "commentkey", url: "/docs/paper#k=commentkey", since: "2026-01-02T00:00:00Z", until: "2027-03-17T00:00:00Z", expired: false, budget: null },
    { id: "a000000000000003", role: "commenter", name: "A very long name that has to be cut short with an ellipsis in a narrow column", key: "k3", url: "/docs/paper#k=k3", since: "2026-01-03T00:00:00Z", until: "", expired: false, budget: 5 },
    { id: "a000000000000004", role: "editor", name: "Old draft", key: "k4", url: "/docs/paper#k=k4", since: "2025-01-01T00:00:00Z", until: "2026-10-02T00:00:00Z", expired: true, budget: null },
  ],
  tokens: [
    { id: "b000000000000001", role: "commenter", name: "Paul's assistant", since: "2026-01-01T00:00:00Z", until: "2027-03-17T00:00:00Z", expired: false, budget: null },
  ],
};
window.calls = [];
const expiryOf = (until) => (until === "never" ? "" : week());
const answer = (created) => ({
  slug: "paper", url: "/docs/paper", can_share: true, publishers: "anyone", commenters_policy: "anyone",
  links: state.links, tokens: state.tokens, ...(created ? { created } : {}),
});
const rowOf = (id) => [...state.links, ...state.tokens].find((row) => row.id === id);
window.fetch = async (path, options) => {
  const body = options?.body ? JSON.parse(options.body) : null;
  window.calls.push([path, body]);
  let created;
  if (body?.create) {
    const { kind, role, name, until, budget } = body.create;
    const id = idOf();
    const row = { id, role, name, since: new Date().toISOString(), until: expiryOf(until), expired: false, budget };
    if (kind === "link") {
      state.links.push({ ...row, key: "minted" + id, url: "/docs/paper#k=minted" + id });
      created = { id, kind, url: "/docs/paper#k=minted" + id };
    } else {
      state.tokens.push(row);
      created = { id, kind, token: "lpt_paper.minted" + id };
    }
  }
  if (body?.update) {
    const row = rowOf(body.update.id);
    if (body.update.name !== undefined) row.name = body.update.name;
    if (body.update.until) { row.until = expiryOf(body.update.until); row.expired = false; }
    row.budget = body.update.budget ?? null;
  }
  if (body?.replace) {
    const row = rowOf(body.replace.id);
    const id = idOf();
    row.id = id;
    if (body.replace.until) { row.until = expiryOf(body.replace.until); row.expired = false; }
    if (row.url !== undefined) {
      row.key = "rotated" + id; row.url = "/docs/paper#k=rotated" + id;
      created = { id, kind: "link", url: row.url };
    } else created = { id, kind: "token", token: "lpt_paper.rotated" + id };
  }
  if (body?.revoke) {
    state.links = state.links.filter((row) => row.id !== body.revoke.id);
    state.tokens = state.tokens.filter((row) => row.id !== body.revoke.id);
  }
  return { ok: true, json: async () => answer(created) };
};
Object.defineProperty(navigator, "clipboard", {
  configurable: true, value: { writeText: async (value) => { window.copied = value; } },
});

const mount = () => createClassComponent({ component: Share, target: document.body, props: {
  open: true, inline: true, slug: "paper", canShare: true,
} });
mount();
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };
const squash = (node) => node.innerText.replace(/\\s+/g, " ").trim();
const text = () => squash(document.body);
const rows = () => [...document.querySelectorAll(".share-row")];
const rowText = (index) => squash(rows()[index]);
const button = (label) => [...document.querySelectorAll("button")]
  .find((node) => (node.getAttribute("aria-label") || node.textContent).trim() === label);
const labelled = (label) => document.querySelector('[aria-label="' + label + '"]');
const press = async (node) => {
  check(node, "missing control");
  node.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "mouse" }));
  node.click();
  await flush();
};
const pick = async (trigger, says) => {
  await press(labelled(trigger));
  const item = [...document.querySelectorAll('[role="menuitem"]')]
    .find((node) => node.textContent.trim() === says && node.getClientRects().length);
  check(item, "missing menu item " + says + " in " + text());
  item.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, pointerType: "mouse" }));
  item.click();
  await flush();
};
const type = async (node, value) => { node.value = value; node.dispatchEvent(new Event("input", { bubbles: true })); node.dispatchEvent(new Event("change", { bubbles: true })); await flush(); };
const tabButton = (label) => [...document.querySelectorAll('[role="tab"]')].find((node) => node.textContent.trim() === label);

window.sharePanelReady = () => rows().length > 0;

window.sharePanelCheck = async () => {
  await flush();
  const panel = document.querySelector(".share-sidebar");

  /* ------------------------------------------------ the links tab at rest */

  check([...document.querySelectorAll('[role="tab"]')].map((node) => node.textContent.trim()).join("|") === "Links|Agent tokens",
    "two tabs: " + text());
  check(text().includes("3 links") && !text().includes("4 links"), "the count is of active links only: " + text());
  check(rows().length === 3, "three active links, the expired one is folded away");
  check(rowText(0).startsWith("Read link Read") && rowText(0).includes("Mar 17"),
    "an unnamed link is called by what it grants, with its expiry: " + rowText(0));
  check(rowText(1).includes("Reviewer") && rowText(1).includes("Comment \\u00b7 Mar 17"), "a named link: " + rowText(1));
  check(rowText(2).includes("Comment \\u00b7 No expiry"), "a link without an end: " + rowText(2));
  check(/[\\u2013\\u2014]/.test(text()) === false, "no dashes in the panel text");
  check(rows().every((row) => row.getBoundingClientRect().height >= 40 && row.getBoundingClientRect().height <= 52), "rows are 44 to 48px tall: "
    + rows().map((row) => Math.round(row.getBoundingClientRect().height)).join(","));
  check(rows()[2].querySelector(".share-name").title.startsWith("A very long name"), "the full name is in a tooltip");
  check(rows()[2].querySelector(".share-name").scrollWidth > rows()[2].querySelector(".share-name").clientWidth, "and it is cut with an ellipsis");
  check(panel.scrollWidth <= panel.clientWidth + 1, "no horizontal scroll");
  check(!labelled("Copy Read link"), "a link with no URL has nothing to copy");
  check(Boolean(labelled("Copy Reviewer")), "a link with a URL can be copied");
  check(!document.querySelector(".share-form"), "no form until one is asked for");

  await press(labelled("Copy Reviewer"));
  check(window.copied === location.origin + "/docs/paper#k=commentkey", "copies the full URL: " + window.copied);

  /* ---------------------------------------------------------- the More menu */

  await press(labelled("More actions for Read link"));
  const items = [...document.querySelectorAll('[role="menuitem"]')].filter((node) => node.getClientRects().length).map((node) => node.textContent.trim());
  check(items.join("|") === "Edit settings|Replace link|Revoke link", "the menu: " + items.join("|"));
  const menu = document.querySelector('[role="menu"][data-state="open"]').getBoundingClientRect();
  check(menu.left >= 0 && menu.right <= innerWidth && menu.bottom <= innerHeight, "the menu stays inside the viewport");
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await flush();

  /* ------------------------------------------------------------ a new link */

  await press(document.querySelector("[data-share-new]"));
  check(document.querySelectorAll(".share-form").length === 1 && !document.querySelector("[data-share-new]"),
    "the form stands in for the toolbar");
  check(document.activeElement === document.querySelector(".share-form input"), "focus moves into the form");
  check(text().includes("A private name to help you remember who you shared this link with."), "the helper line");
  const form = document.querySelector(".share-form");
  await type(form.querySelector("input[type=text]"), "For Ana");
  const selects = form.querySelectorAll("select");
  check(selects[1].value === "7d", "seven days is the default");
  await type(selects[0], "editor");
  await type(form.querySelector("input[type=number]"), "0");
  await press(button("Create link"));
  const made = window.calls.at(-1)[1].create;
  check(made.kind === "link" && made.role === "editor" && made.name === "For Ana" && made.until === "7d" && made.budget === 0,
    "the link that was described is created, with a budget of zero kept: " + JSON.stringify(made));
  check(rows().length === 4 && text().includes("4 links"), "created without replacing another: " + text());
  check(document.activeElement === document.querySelector("[data-share-new]") || document.activeElement === document.body,
    "focus returns to the toolbar");

  /* -------------------------------------------------------------- settings */

  await pick("More actions for Reviewer", "Edit settings");
  check(document.querySelectorAll(".share-form").length === 1 && document.querySelector("[data-share-new]"),
    "the form stands in for the row, the toolbar stays");
  check(!document.querySelector(".share-form select[disabled]") && document.querySelectorAll(".share-form select").length === 1,
    "permission is fixed, only the expiry is chosen");
  check(document.querySelector(".share-form select option").textContent.startsWith("Unchanged (Mar 17, 2027)"), "unchanged is offered first");
  await press(document.querySelector("[data-share-new]"));
  check(document.querySelectorAll(".share-form").length === 1, "only one form is open at a time");
  await press(button("Cancel"));
  check(!document.querySelector(".share-form"), "cancel closes it");

  await pick("More actions for Reviewer", "Edit settings");
  await type(document.querySelector(".share-form input[type=text]"), "Reviewer 2");
  await press(button("Save"));
  const updated = window.calls.at(-1)[1].update;
  check(updated.id === "a000000000000002" && updated.name === "Reviewer 2" && updated.until === "",
    "an update keeps the expiry and sends the name: " + JSON.stringify(updated));

  /* ---------------------------------------------------------- confirmations */

  await pick("More actions for Read link", "Replace link");
  check(text().includes("Replace link?") && text().includes("The current link will stop working. Anyone using it will need the new link."), "replace asks first");
  const before = window.calls.length;
  await press(button("Cancel"));
  check(window.calls.length === before, "cancelling sends nothing");
  await pick("More actions for Read link", "Replace link");
  await press([...document.querySelectorAll('[role="dialog"] button')].find((node) => node.textContent.trim() === "Replace link"));
  check(Object.keys(window.calls.at(-1)[1]).join() === "replace", "replace is its own action");
  await pick("More actions for For Ana", "Revoke link");
  check(text().includes("Revoke link?") && text().includes("People who only have access through this link will lose access."), "revoke asks first");
  await press([...document.querySelectorAll('[role="dialog"] button')].find((node) => node.textContent.trim() === "Revoke link"));
  check(window.calls.at(-1)[1].revoke && rows().length === 3, "revoked: " + text());

  /* ---------------------------------------------------------------- expired */

  check(text().includes("Expired (1)") && !text().includes("Old draft"), "expired rows are folded away");
  await press(button("Expired (1)"));
  check(text().includes("Old draft") && text().includes("Edit \\u00b7 Expired Oct 2"), "expired rows say so in words: " + text());
  await press(labelled("More actions for Old draft"));
  const lapsed = [...document.querySelectorAll('[role="menuitem"]')].filter((node) => node.getClientRects().length).map((node) => node.textContent.trim());
  check(lapsed.join("|") === "Replace link|Remove", "the expired menu: " + lapsed.join("|"));
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await flush();

  /* ------------------------------------------------------------ agent tokens */

  await press(tabButton("Agent tokens"));
  check(text().includes("1 token") && !text().includes("1 tokens"), "one token: " + text());
  check(rowText(0).includes("Paul's assistant") && rowText(0).includes("Comment \\u00b7 Mar 17"), "token row: " + rowText(0));
  check(!document.querySelector('[aria-label^="Copy"]'), "a token has no copy button");
  check(rows()[0].querySelector("svg"), "and an icon");
  await press(document.querySelector("[data-share-new]"));
  check(button("Create token").disabled, "a token needs a name");
  check(![...document.querySelectorAll(".share-form option")].some((node) => node.textContent === "Never"), "and cannot be made to last forever");
  check(text().includes("Its comments and suggestions appear under this name."), "the helper line");
  const tokenForm = document.querySelector(".share-form");
  check(tokenForm.closest(".share-scroll"), "the new form scrolls with the list, so a short panel reaches its buttons");
  await type(tokenForm.querySelector("input[type=text]"), "Codex");
  const roleSelect = tokenForm.querySelectorAll("select")[0];
  const permissions = [...roleSelect.options].map((node) => node.textContent).join("|");
  check(permissions === "Read|Comment|Edit", "the token permissions: " + permissions);
  check(!text().includes("Edit lets the agent change the project directly, without review."), "no warning until Edit is chosen");
  await type(roleSelect, "editor");
  check(text().includes("Edit lets the agent change the project directly, without review."), "choosing Edit shows the warning: " + text());
  check(tokenForm.querySelectorAll("select")[1].value === "1d", "choosing Edit moves the expiry to one day");
  await press(button("Create token"));
  const token = window.calls.at(-1)[1].create;
  check(token.kind === "token" && token.role === "editor" && token.name === "Codex" && token.until === "1d", JSON.stringify(token));

  /* ---------------------------------------------------------- token reveal */

  const secret = document.querySelector('input[aria-label="Agent token"]');
  check(secret && secret.readOnly && secret.value.startsWith("lpt_paper.minted"), "the token is shown: " + text());
  check(text().includes("Copy this token now. You won't be able to view it again."), "with the warning");
  const config = document.querySelector('textarea[aria-label="MCP configuration"]').value;
  check(config.includes('args = ["mcp", "--document", "' + location.origin + '/docs/paper"]') && config.includes('LIBREPAPER_AGENT_TOKEN = "' + secret.value + '"'),
    "the configuration is filled in: " + config);
  check(text().includes("POST /api/documents/{slug}/tools/{name}"), "the plain HTTP route is mentioned");
  check([...document.querySelectorAll("a")].some((node) => node.textContent === "Agent setup" && node.href.includes("agents.html")), "and the setup page is linked");
  const shownToken = secret.value;
  await press(button("Copy token"));
  check(window.copied === shownToken, "copies the token");
  await press(button("Copy configuration"));
  check(window.copied === config, "copies the configuration");
  const stored = JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage });
  check(!stored.includes(shownToken), "the token is stored nowhere");
  await press(button("Done"));
  check(!document.querySelector('input[aria-label="Agent token"]') && !text().includes(shownToken), "Done clears it");
  check(text().includes("Codex") && text().includes("2 tokens"), "and the token is in the list: " + text());

  await pick("More actions for Codex", "Replace token");
  check(text().includes("Replace token?") && text().includes("The agent will need the new token."), "replacing a token asks first");
  await press([...document.querySelectorAll('[role="dialog"] button')].find((node) => node.textContent.trim() === "Replace token"));
  const rotated = document.querySelector('input[aria-label="Agent token"]')?.value || "";
  check(rotated.startsWith("lpt_paper.rotated"), "a replaced token is shown once too");
  const clipboard = navigator.clipboard.writeText;
  navigator.clipboard.writeText = async () => { throw new Error("blocked"); };
  await press(button("Copy token"));
  check(document.querySelector('textarea[aria-label="Text to copy manually"]')?.value === rotated, "a blocked copy offers the token to copy by hand");
  navigator.clipboard.writeText = clipboard;
  await press(button("Done"));
  check(!document.querySelector('textarea[aria-label="Text to copy manually"]') && !text().includes(rotated), "Done also clears the manual copy");
  await pick("More actions for Codex", "Replace token");
  await press([...document.querySelectorAll('[role="dialog"] button')].find((node) => node.textContent.trim() === "Replace token"));
  const late = document.querySelector('input[aria-label="Agent token"]')?.value || "";
  let refuse;
  navigator.clipboard.writeText = () => new Promise((resolve, reject) => { refuse = reject; });
  await press(button("Copy token"));
  await press(button("Done"));
  refuse(new Error("blocked"));
  await flush();
  navigator.clipboard.writeText = clipboard;
  check(!document.querySelector('textarea[aria-label="Text to copy manually"]') && !text().includes(late), "a copy that fails after Done does not bring the token back");
  await pick("More actions for Codex", "Revoke token");
  check(text().includes("Revoke token?") && text().includes("The agent loses access at once."), "revoking a token asks first");
  await press([...document.querySelectorAll('[role="dialog"] button')].find((node) => node.textContent.trim() === "Revoke token"));
  check(rows().length === 1, "revoked");
  await press(tabButton("Links"));
  check(!text().includes("No sharing links yet."), "links are still there");
  return true;
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "share-panel-check.js", cssFileName: "style" } } });
  assertFreshDist("share-panel-browser");
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
  console.log("share panel: several links per role, names, the More menu, confirmations, tokens and the one-time reveal, expired rows");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
