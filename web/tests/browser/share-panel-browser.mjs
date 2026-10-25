// Browser regression check for the sharing panel: links and agent tokens as
// two lists of two-line rows with copy and trash buttons, the revoke confirmations, the one-time
// token screen.
//
// The route is stubbed rather than run, because what is under test is the
// panel, not the server's writes. The stub follows the share contract: every
// live row comes back in `links` or `tokens`, expired rows never do,
// and create adds a `created` object. One link has no URL on
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
const originPart = btoa(location.origin).split("+").join("-").split("/").join("_").split("=").join("");

const state = {
  links: [
    { id: "a000000000000001", role: "reader", name: "", key: "", url: "", since: "2026-01-01T00:00:00Z", until: "2027-03-17T00:00:00Z", budget: null },
    { id: "a000000000000002", role: "commenter", name: "Reviewer", key: "commentkey", url: "/docs/paper#k=commentkey", since: "2026-01-02T00:00:00Z", until: "2027-03-17T00:00:00Z", budget: null },
    { id: "a000000000000003", role: "commenter", name: "A very long name that has to be cut short with an ellipsis in a narrow column", key: "k3", url: "/docs/paper#k=k3", since: "2026-01-03T00:00:00Z", until: "", budget: 5 },
  ],
  tokens: [
    { id: "b000000000000001", role: "commenter", name: "Paul's assistant", since: "2026-01-01T00:00:00Z", until: "2027-03-17T00:00:00Z", budget: null },
  ],
};
window.calls = [];
const expiryOf = (until) => (until === "never" ? "" : week());
const answer = (created) => ({
  slug: "paper", url: "/docs/paper", can_share: true, publishers: "anyone", commenters_policy: "anyone",
  links: state.links, tokens: state.tokens, ...(created ? { created } : {}),
});
window.fetch = async (path, options) => {
  const body = options?.body ? JSON.parse(options.body) : null;
  window.calls.push([path, body]);
  let created;
  if (body?.create) {
    const { kind, role, name, until, budget } = body.create;
    const id = idOf();
    const row = { id, role, name, since: new Date().toISOString(), until: expiryOf(until), budget };
    if (kind === "link") {
      state.links.push({ ...row, key: "minted" + id, url: "/docs/paper#k=minted" + id });
      created = { id, kind, url: "/docs/paper#k=minted" + id };
    } else {
      state.tokens.push(row);
      created = { id, kind, token: "lpt_" + originPart + ".paper.minted" + id };
    }
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
const type = async (node, value) => { node.value = value; node.dispatchEvent(new Event("input", { bubbles: true })); node.dispatchEvent(new Event("change", { bubbles: true })); await flush(); };
const tabButton = (label) => [...document.querySelectorAll('[role="tab"]')].find((node) => node.textContent.trim() === label);

window.sharePanelReady = () => rows().length > 0;

window.sharePanelCheck = async () => {
  await flush();
  const panel = document.querySelector(".share-sidebar");

  /* ------------------------------------------------ the links tab at rest */

  check([...document.querySelectorAll('[role="tab"]')].map((node) => node.textContent.trim()).join("|") === "Links|Agent tokens",
    "two tabs: " + text());
  check(!/\\d+ (links?|tokens?)/.test(squash(document.querySelector(".share-toolbar"))), "the toolbar has no count");
  check(squash(document.querySelector(".share-toolbar")) === "New", "only the New button: " + squash(document.querySelector(".share-toolbar")));
  check(rows().length === 3 && !text().includes("Expired"), "three links and no expired section");
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
  const actionsOf = (row) => [...row.querySelectorAll(".share-actions button")].map((node) => node.getAttribute("aria-label"));
  check(!labelled("Copy Read link"), "a link with no URL has nothing to copy");
  check(actionsOf(rows()[0]).join("|") === "Revoke Read link", "a link with no URL has only trash: " + actionsOf(rows()[0]));
  check(actionsOf(rows()[1]).join("|") === "Copy Reviewer|Revoke Reviewer", "a link has copy and trash: " + actionsOf(rows()[1]));
  check(!document.querySelector('[aria-label^="More actions"]') && !document.querySelector('[role="menu"]'), "there is no More menu");
  check(!document.querySelector(".share-form"), "no form until one is asked for");

  await press(labelled("Copy Reviewer"));
  check(window.copied === location.origin + "/docs/paper#k=commentkey", "copies the full URL: " + window.copied);
  check(!text().includes("copied"), "copying prints no status line: " + text());

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
  check(rows().length === 4, "created without replacing another: " + text());
  check(document.activeElement === document.querySelector("[data-share-new]") || document.activeElement === document.body,
    "focus returns to the toolbar");

  /* ------------------------------------------------------------- revoking */

  const confirmButton = (says) => [...document.querySelectorAll('[role="dialog"] button')].find((node) => node.textContent.trim() === says);
  const before = window.calls.length;
  await press(labelled("Revoke Reviewer"));
  check(text().includes("Revoke link?") && text().includes("People who only have access through this link will lose access."), "trash asks first");
  check(window.calls.length === before, "and sends nothing yet");
  await press(button("Cancel"));
  check(window.calls.length === before && rows().length === 4 && text().includes("Reviewer"), "Cancel keeps the row and sends nothing");
  check(document.activeElement === document.querySelector("[data-share-new]"), "focus goes to New after Cancel");
  await press(labelled("Revoke For Ana"));
  await press(confirmButton("Revoke link"));
  check(JSON.stringify(window.calls.at(-1)[1]) === JSON.stringify({ revoke: { id: window.calls.at(-1)[1].revoke.id } }), "revoke is its own action: " + JSON.stringify(window.calls.at(-1)[1]));
  check(rows().length === 3 && !text().includes("For Ana"), "the row is gone: " + text());
  check(!text().includes("revoked") && !text().includes("Revoked"), "and no status line says so: " + text());
  check(document.activeElement === document.querySelector("[data-share-new]"), "focus goes to New after a revoke");

  /* ------------------------------------------------------------ agent tokens */

  await press(tabButton("Agent tokens"));
  check(rows().length === 1 && squash(document.querySelector(".share-toolbar")) === "New", "one token and no count: " + text());
  check(rowText(0).includes("Paul's assistant") && rowText(0).includes("Comment \\u00b7 Mar 17"), "token row: " + rowText(0));
  check(!document.querySelector('[aria-label^="Copy"]'), "a token has no copy button");
  check([...rows()[0].querySelectorAll(".share-actions button")].map((node) => node.getAttribute("aria-label")).join("|") === "Revoke Paul's assistant", "a token has only trash");
  check(rows()[0].querySelector("svg"), "and an icon");
  check(text().includes("A token with the Edit role is also what the librepaper command line and the GitHub Action use"), "the tokens tab says what an Edit token is for: " + text());
  await press(document.querySelector("[data-share-new]"));
  check(button("Create token").disabled, "a token needs a name");
  check(![...document.querySelectorAll(".share-form option")].some((node) => node.textContent === "Never"), "and cannot be made to last forever");
  check(text().includes("Its comments and suggestions appear under this name."), "the helper line");
  const tokenForm = document.querySelector(".share-form");
  check(tokenForm.closest(".share-scroll"), "the new form scrolls with the list, so a short panel reaches its buttons");
  await type(tokenForm.querySelector("input[type=text]"), "Codex");
  const roleSelect = tokenForm.querySelectorAll("select")[0];
  const expiries = [...tokenForm.querySelectorAll("select")[1].options].map((node) => node.textContent).join("|");
  check(expiries === "1 day|7 days|30 days|6 months|1 year", "a token can last up to a year: " + expiries);
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
  check(secret && secret.readOnly && secret.value.startsWith("lpt_" + originPart + ".paper.minted"), "the token is shown: " + text());
  check(text().includes("Copy this token now. You won't be able to view it again."), "with the warning");
  const commands = document.querySelector('textarea[aria-label="Claude Code commands"]').value;
  check(commands === "claude mcp add librepaper --scope user -- librepaper mcp\\nread -rs LIBREPAPER_AGENT_TOKEN && export LIBREPAPER_AGENT_TOKEN && claude",
    "the Claude Code commands register once and read the token silently: " + commands);
  check(!commands.includes(secret.value), "the commands never contain the token");
  const config = document.querySelector('textarea[aria-label="MCP configuration"]').value;
  check(config.includes('args = ["mcp"]') && config.includes('LIBREPAPER_AGENT_TOKEN = "' + secret.value + '"'),
    "the configuration is filled in: " + config);
  check(text().includes("POST /api/documents/{slug}/tools/{name}"), "the plain HTTP route is mentioned");
  check([...document.querySelectorAll("a")].some((node) => node.textContent === "Agent setup" && node.href.includes("agents.html")), "and the setup page is linked");
  const shownToken = secret.value;
  await press(button("Copy token"));
  check(window.copied === shownToken, "copies the token");
  await press(button("Copy commands"));
  check(window.copied === commands, "copies the commands");
  await press(button("Copy configuration"));
  check(window.copied === config, "copies the configuration");
  const stored = JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage });
  check(!stored.includes(shownToken), "the token is stored nowhere");
  await press(button("Done"));
  check(!document.querySelector('input[aria-label="Agent token"]') && !text().includes(shownToken), "Done clears it");
  check(text().includes("Codex") && rows().length === 2, "and the token is in the list: " + text());

  await press(labelled("Revoke Codex"));
  check(text().includes("Revoke token?") && text().includes("The agent loses access at once."), "revoking a token asks first");
  await press(confirmButton("Revoke token"));
  check(rows().length === 1 && !text().includes("Codex"), "revoked");
  check(!text().includes("revoked") && !text().includes("copied"), "and no status line says so");
  await press(document.querySelector("[data-share-new]"));
  await type(document.querySelector(".share-form input[type=text]"), "Second");
  await press(button("Create token"));
  const rotated = document.querySelector('input[aria-label="Agent token"]')?.value || "";
  const clipboard = navigator.clipboard.writeText;
  navigator.clipboard.writeText = async () => { throw new Error("blocked"); };
  await press(button("Copy token"));
  check(document.querySelector('textarea[aria-label="Text to copy manually"]')?.value === rotated, "a blocked copy offers the token to copy by hand");
  navigator.clipboard.writeText = clipboard;
  await press(button("Done"));
  check(!document.querySelector('textarea[aria-label="Text to copy manually"]') && !text().includes(rotated), "Done also clears the manual copy");
  await press(document.querySelector("[data-share-new]"));
  await type(document.querySelector(".share-form input[type=text]"), "Third");
  await press(button("Create token"));
  const late = document.querySelector('input[aria-label="Agent token"]')?.value || "";
  let refuse;
  navigator.clipboard.writeText = () => new Promise((resolve, reject) => { refuse = reject; });
  await press(button("Copy token"));
  await press(button("Done"));
  refuse(new Error("blocked"));
  await flush();
  navigator.clipboard.writeText = clipboard;
  check(!document.querySelector('textarea[aria-label="Text to copy manually"]') && !text().includes(late), "a copy that fails after Done does not bring the token back");
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
  console.log("share panel: several links per role, names, copy and trash rows, revoke confirmations, tokens and the one-time reveal");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
