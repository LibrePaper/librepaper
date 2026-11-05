// Browser regression check for the Agent panel. An owner gets two tabs: Tokens,
// where a token is made, revealed once in the panel and revoked, and Tasks,
// where each instruction is copied. A viewer who cannot share gets the tasks
// alone, with no tab row, and loads no sharing state.
//
// The route is stubbed rather than run, because what is under test is the
// panel, not the server's writes. The stub follows the share contract: every
// live row comes back in `links` or `tokens`, and create adds a `created`
// object.
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

process.env.TZ = "UTC";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-agent-panel-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import Agent from ${JSON.stringify(join(root, "web/src/components/reader/Agent.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

let counter = 0;
const idOf = () => String(++counter).padStart(16, "0");
const week = () => new Date(Date.now() + 7 * 86400000).toISOString();
const originPart = btoa(location.origin).split("+").join("-").split("/").join("_").split("=").join("");

const state = {
  links: [],
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
    state.tokens.push(row);
    created = { id, kind, token: "lpt_" + originPart + ".paper.minted" + id };
    window.issuedToken = created.token;
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

const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };
const squash = (node) => node.innerText.replace(/\\s+/g, " ").trim();
const text = () => squash(document.body);
const rows = () => [...document.querySelectorAll(".share-row")];
const rowText = (index) => squash(rows()[index]);
const button = (label) => [...document.querySelectorAll("button")]
  .find((node) => (node.getAttribute("aria-label") || node.textContent).trim() === label);
const labelled = (label) => document.querySelector('[aria-label="' + label + '"]');
const tabButton = (label) => [...document.querySelectorAll('[role="tab"]')].find((node) => node.textContent.trim() === label);
const press = async (node) => {
  check(node, "missing control");
  node.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "mouse" }));
  node.click();
  await flush();
};
const type = async (node, value) => { node.value = value; node.dispatchEvent(new Event("input", { bubbles: true })); node.dispatchEvent(new Event("change", { bubbles: true })); await flush(); };
const dialog = () => document.querySelector('[role="dialog"]');
const closed = async () => {
  for (let attempt = 0; attempt < 100 && dialog(); attempt++) await new Promise((resolve) => setTimeout(resolve, 20));
  check(!dialog(), "the dialog closes");
};
const confirmButton = (says) => [...document.querySelectorAll('[role="dialog"] button')].find((node) => node.textContent.trim() === says);
const copiedLabel = () => [...document.querySelector(".agent-panel").querySelectorAll("button")].some((node) => node.textContent.trim() === "Copied");

const mount = (canShare) => createClassComponent({ component: Agent, target: document.body, props: {
  open: true, slug: "paper", canShare,
} });
let owner = mount(true);

window.agentPanelReady = () => Boolean(document.querySelector("[data-share-new]"));

const viewerChecks = async () => {
  const before = window.calls.length;
  owner.$destroy();
  owner = null;
  const viewer = mount(false);
  await flush();
  const panel = document.querySelector(".agent-panel");
  check(!document.querySelector('[role="tab"]'), "a viewer sees no tab row: " + text());
  check(!document.querySelector("[data-share-new]"), "a viewer sees no New button");
  check(!text().includes("Paul's assistant"), "a viewer sees no tokens");
  check(window.calls.length === before, "a viewer loads no sharing state");
  check(document.querySelectorAll(".agent-task").length === 12, "a viewer sees the tasks: " + document.querySelectorAll(".agent-task").length);
  check(text().includes("Copy a task and paste it into your agent."), "the tasks intro is the same for a viewer");
  check(document.querySelector(".agent-task-search")?.placeholder === "Search tasks", "a viewer sees the search box");
  check(panel.scrollWidth <= panel.clientWidth + 1, "a viewer's panel has no horizontal scroll");
  await press(labelled("Copy Outline"));
  check(window.copied === "Outline this LibrePaper project: list the headings and say what each section does. Do not change the project.",
    "a viewer copies a task: " + window.copied);
  viewer.$destroy();
  return true;
};

window.agentPanelCheck = async () => {
  await flush();
  const panel = document.querySelector(".agent-panel");
  const tabs = () => [...document.querySelectorAll('[role="tab"]')];
  const tabNames = () => tabs().map((node) => node.textContent.trim()).join("|");
  const selectedTab = () => tabs().find((node) => node.getAttribute("aria-selected") === "true")?.textContent.trim();

  /* ------------------------------------------------- the tokens tab at rest */

  check(tabNames() === "Tokens|Tasks", "two tabs, Tokens first: " + tabNames());
  check(selectedTab() === "Tokens", "Tokens is the default tab: " + selectedTab());
  check(!dialog(), "no dialog at rest");
  check(text().includes("Create a token for an agent you run yourself, such as Claude Code or Codex. It reaches only this project."), "the tokens intro");
  check(squash(document.querySelector(".share-toolbar")) === "New", "only the New button: " + squash(document.querySelector(".share-toolbar")));
  check(rows().length === 1 && rowText(0).includes("Paul's assistant") && rowText(0).includes("Comment \\u00b7 Mar 17"), "one token row: " + rowText(0));
  check(!text().includes("lpt_"), "no token is visible by default");
  check([...rows()[0].querySelectorAll(".share-actions button")].map((node) => node.getAttribute("aria-label")).join("|") === "Revoke Paul's assistant",
    "a token has only trash");
  check(!rows()[0].querySelector('[aria-label^="Copy"]'), "a token has no copy button");
  check(text().includes("A token with the Edit role is also what the librepaper command line and the GitHub Action use to sync this project."),
    "the scope note: " + text());
  check(!document.querySelector(".share-form"), "no form until one is asked for");
  check(panel.scrollWidth <= panel.clientWidth + 1, "no horizontal scroll");

  /* ------------------------------------------------------------ a new token */

  await press(document.querySelector("[data-share-new]"));
  check(!button("Create token").disabled, "a token needs no name");
  check(![...document.querySelectorAll(".share-form option")].some((node) => node.textContent === "Never"), "and cannot be made to last forever");
  const form = document.querySelector(".share-form");
  check(!form.querySelector("details"), "the token form has no folded section");
  const nameInput = form.querySelector("input[type=text]");
  check(nameInput.getClientRects().length > 0 && nameInput.value === "" && nameInput.placeholder === "(optional)",
    "the name is visible, empty, and shows (optional): " + nameInput.value + " / " + nameInput.placeholder);
  const budgetInput = form.querySelector("input[type=number]");
  check(budgetInput && budgetInput.getClientRects().length > 0, "the Comments/hour field shows for the default Comment role");
  check(text().includes("Its comments and suggestions appear under this name."), "the helper line");
  check(document.querySelector(".share-toolbar") === null, "the form stands in for the toolbar");
  check(document.activeElement === form.querySelectorAll("select")[0], "a token's first control is its permission");
  check(form.querySelectorAll("select")[0].value === "commenter" && form.querySelectorAll("select")[1].value === "7d",
    "a token starts as Comment for seven days");
  await type(form.querySelector("input[type=text]"), "Codex");
  const roleSelect = form.querySelectorAll("select")[0];
  const expiries = [...form.querySelectorAll("select")[1].options].map((node) => node.textContent).join("|");
  check(expiries === "1 day|7 days|30 days|6 months|1 year", "a token can last up to a year: " + expiries);
  const permissions = [...roleSelect.options].map((node) => node.textContent).join("|");
  check(permissions === "Read|Comment|Edit", "the token permissions: " + permissions);
  check(!text().includes("Edit lets the agent suggest tracked changes, and also change the project directly."), "no warning until Edit is chosen");
  await type(roleSelect, "editor");
  check(text().includes("Edit lets the agent suggest tracked changes, and also change the project directly."), "choosing Edit shows the warning");
  check(form.querySelectorAll("select")[1].value === "1d", "choosing Edit moves the expiry to one day");
  await press(button("Create token"));
  const made = window.calls.at(-1)[1].create;
  check(made.kind === "token" && made.role === "editor" && made.name === "Codex" && made.until === "1d", JSON.stringify(made));

  /* ------------------------------------------------------- the reveal, once */

  // The token is read from the mock, which issued it, or from the text that
  // was copied. It is never read back from the page to check the page.
  const issued = window.issuedToken;
  check(issued && issued.startsWith("lpt_" + originPart + ".paper.minted"), "the mock issued a token");
  check(!dialog(), "the reveal is not a dialog");
  check(panel.contains(button("Copy install command")), "the reveal is drawn in the panel");
  check(text().includes("Connect your AI agent"), "the reveal heading: " + text());
  check(text().includes("Your token is shown only once."), "with the warning");
  check(rows().length === 0 && !document.querySelector("[data-share-new]"), "the reveal replaces the toolbar and list");
  check(!panel.textContent.includes(issued) && [...panel.querySelectorAll("textarea, input")].every((node) => !node.value.includes(issued)),
    "by default no field and no text holds the token");
  check(document.activeElement === button("Copy install command"), "focus opens on the install copy button, not Done: " + (document.activeElement && document.activeElement.textContent));
  await press(button("Copy install command"));
  check(window.copied.includes("npx skills add LibrePaper/librepaper --skill librepaper --global")
    && window.copied.includes("https://github.com/LibrePaper/librepaper/tree/main/skills/librepaper")
    && !window.copied.includes(issued), "the install command copies without the token: " + window.copied);
  check(copiedLabel(), "a copy button shows Copied afterwards");
  check(panel.contains(button("Copy connection instructions")), "copying does not end the reveal");
  await press(button("Copy connection instructions"));
  check(window.copied.includes("Use the LibrePaper skill") && window.copied.includes("no local folder") && window.copied.includes("Do not open or control a browser unless I explicitly ask you to.")
    && window.copied.includes("Token: " + issued), "the connection instructions copy with the token: " + window.copied);

  await press(panel.querySelector("summary"));
  check(panel.querySelector("details").open, "Other connection methods opens");
  check(text().includes("With MCP, you can avoid pasting the token into your agent's chat."), "the MCP advantage line");
  const commands = labelled("Claude Code commands").value;
  check(commands === "claude mcp add librepaper --scope user -- librepaper mcp\\nread -rs LIBREPAPER_AGENT_TOKEN && export LIBREPAPER_AGENT_TOKEN && claude",
    "the Claude Code commands register once and read the token silently: " + commands);
  check(!commands.includes(issued), "the commands never contain the token");
  await press(button("Copy commands"));
  check(window.copied === commands, "copies the commands");
  const config = labelled("MCP configuration").value;
  check(config.includes('args = ["mcp"]') && config.includes('LIBREPAPER_AGENT_TOKEN = "lpt_..."') && !config.includes(issued),
    "the configuration shows lpt_... until the token is revealed: " + config);
  await press(button("Copy configuration"));
  check(window.copied.includes('args = ["mcp"]') && window.copied.includes('LIBREPAPER_AGENT_TOKEN = "' + issued + '"'),
    "Copy configuration still copies the real token: " + window.copied);
  check(!labelled("Agent token"), "the token field is not drawn until it is revealed");
  check(text().includes("POST /api/documents/{slug}/tools/{name}"), "the plain HTTP route is mentioned");
  check([...panel.querySelectorAll("a")].some((node) => node.textContent === "Agent setup" && node.href.includes("agents.html")), "and the setup page is linked");

  await press(button("Reveal token"));
  check(labelled("Agent token") && labelled("Agent token").value === issued, "after Reveal the input shows the token");
  check(button("Hide token") && button("Hide token").getAttribute("aria-expanded") === "true", "the toggle says it is expanded");
  await press(button("Copy token"));
  check(window.copied === issued, "copies the token");
  await press(button("Hide token"));
  check(!labelled("Agent token") && !panel.textContent.includes(issued), "Hide takes the token off the screen again");
  await press(button("Reveal token"));
  check(labelled("Agent token").value === issued, "and Reveal shows it once more");

  const stored = JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage });
  check(!stored.includes(issued), "the token is stored nowhere");
  await press(button("Done"));
  check(!labelled("Agent token") && !text().includes(issued), "Done clears it");
  check(rows().length === 2 && rows().some((row) => row.textContent.includes("Codex")), "and the token is in the list: " + text());
  check(document.activeElement === document.querySelector("[data-share-new]"), "focus goes to New after Done");

  /* ---------------------------------------------------------------- revoke */

  await press(labelled("Revoke Codex"));
  check(dialog() && text().includes("Revoke token?") && text().includes("The agent loses access at once."), "revoking a token asks first in a dialog");
  await press(confirmButton("Revoke token"));
  check(rows().length === 1 && !rows()[0].textContent.includes("Codex"), "revoked");
  check(!text().includes("revoked") && !text().includes("copied"), "and no status line says so");

  /* ----------------------------------------------- a manual-copy fallback */

  await press(document.querySelector("[data-share-new]"));
  await type(document.querySelector(".share-form input[type=text]"), "Second");
  await press(button("Create token"));
  const rotated = window.issuedToken;
  const clipboard = navigator.clipboard.writeText;
  navigator.clipboard.writeText = async () => { throw new Error("blocked"); };
  await press(button("Copy token"));
  check(document.querySelector('textarea[aria-label="Text to copy manually"]')?.value === rotated, "a blocked copy offers the token to copy by hand");
  check(button("Could not copy"), "and the button says the copy failed");
  navigator.clipboard.writeText = clipboard;
  await press(button("Done"));
  check(!document.querySelector('textarea[aria-label="Text to copy manually"]') && !text().includes(rotated), "Done also clears the manual copy");

  /* ------------------------------------------------ an unnamed token, late */

  await press(document.querySelector("[data-share-new]"));
  check(!button("Create token").disabled, "a token with no name can still be created");
  await press(button("Create token"));
  check(window.calls.at(-1)[1].create.name === "Agent", "an unnamed token is called Agent: " + JSON.stringify(window.calls.at(-1)[1]));
  const late = window.issuedToken;
  let refuse;
  navigator.clipboard.writeText = () => new Promise((resolve, reject) => { refuse = reject; });
  await press(button("Copy token"));
  await press(button("Done"));
  refuse(new Error("blocked"));
  await flush();
  navigator.clipboard.writeText = clipboard;
  check(!document.querySelector('textarea[aria-label="Text to copy manually"]') && !text().includes(late), "a copy that fails after Done does not bring the token back");

  /* ------------------------------------------------------------------ tasks */

  await press(tabButton("Tasks"));
  check([...document.querySelectorAll(".agent-task-group-label")].map((node) => node.textContent.trim()).join("|") === "Writing|Review|Document",
    "every group heading appears: " + text());
  const taskRows = [...document.querySelectorAll(".agent-task")];
  check(taskRows.length === 12, "twelve tasks: " + taskRows.length);
  check(taskRows.every((row) => row.querySelector(".agent-task-name").textContent.trim() && row.querySelector(".agent-task-body").textContent.trim()
    && row.querySelectorAll("a, input, button, select, textarea").length === 1), "each task has a name, its text and one copy button");
  const proofread = "Proofread this LibrePaper project and suggest the corrections as tracked changes.";
  check(taskRows[0].textContent.includes(proofread), "the proofread task shows its text");
  const iconBefore = labelled("Copy Proofread").innerHTML;
  await press(labelled("Copy Proofread"));
  check(window.copied === proofread, "copies exactly the shown text: " + window.copied);
  check(labelled("Copy Proofread").innerHTML !== iconBefore, "the icon changes once copied");
  check(panel.scrollWidth <= panel.clientWidth + 1, "the tasks have no horizontal scroll");

  /* ----------------------------------------------------- the task search */

  const searchBox = () => document.querySelector(".agent-panel .agent-task-search");
  const heading = (label) => [...document.querySelectorAll(".agent-task-toggle")]
    .find((node) => node.querySelector(".agent-task-group-label").textContent.trim() === label);
  const shownTasks = () => [...document.querySelectorAll(".agent-task")];
  const statusLine = () => document.querySelector('.agent-panel [role="status"]');
  const pressKey = async (node, name) => {
    node.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true }));
    await flush();
  };

  check(searchBox() && searchBox().type === "search" && searchBox().placeholder === "Search tasks", "the tasks have a search box");
  check(document.activeElement !== searchBox(), "the search box is not focused when the panel opens");
  check(["Writing", "Review", "Document"].every((label) => heading(label).getAttribute("aria-expanded") === "true"),
    "every group starts expanded");
  check(heading("Review").getAttribute("aria-controls") === "agent-tasks-review" && document.getElementById("agent-tasks-review"),
    "a heading controls its list");

  await press(heading("Review"));
  check(heading("Review").getAttribute("aria-expanded") === "false" && !document.getElementById("agent-tasks-review"),
    "clicking a heading folds its rows");
  check(shownTasks().length === 6, "the other groups stay open: " + shownTasks().length);
  await press(heading("Review"));
  check(heading("Review").getAttribute("aria-expanded") === "true" && shownTasks().length === 12, "clicking again restores the rows");

  await press(heading("Review"));
  await type(searchBox(), "bibliography");
  check(shownTasks().length === 1 && shownTasks()[0].textContent.includes("Check citations"),
    "a folded group's task shows while searching: " + text());
  check(document.querySelectorAll(".agent-task-toggle").length === 1, "only the matching group is shown");
  check(heading("Review").getAttribute("aria-expanded") === "true" && heading("Review").disabled,
    "the matching group opens and its heading cannot be folded");

  await type(searchBox(), "");
  check(shownTasks().length === 6 && heading("Review").getAttribute("aria-expanded") === "false",
    "clearing the search restores the fold: " + shownTasks().length);
  await press(heading("Review"));
  check(shownTasks().length === 12, "the group is open again after the fold is undone");

  await type(searchBox(), "  zqxw ");
  check(statusLine() && statusLine().textContent.trim() === 'No tasks match "zqxw".',
    "a search with no match says so: " + (statusLine() && statusLine().textContent));
  check(shownTasks().length === 0, "no task matches zqxw");

  await type(searchBox(), "outline");
  check(shownTasks().length === 1, "only the matching row is shown: " + shownTasks().length);
  await press(labelled("Copy Outline"));
  check(window.copied === "Outline this LibrePaper project: list the headings and say what each section does. Do not change the project.",
    "copying a search result copies exactly its text: " + window.copied);

  await pressKey(searchBox(), "Escape");
  check(searchBox().value === "" && shownTasks().length === 12, "Escape in the search box clears it");

  return viewerChecks();
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "agent-panel-check.js", cssFileName: "style" } } });
  assertFreshDist("agent-panel-browser");
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
      + '<body><script type="module" src="/agent-panel-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 24000 + Math.floor(Math.random() * 1000));
  await tab.resize(360, 900);
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("agent panel component", () => tab.evaluate("Boolean(window.agentPanelReady && window.agentPanelReady())"));
  assert.equal(await tab.evaluate("window.agentPanelCheck()"), true);
  console.log("agent panel: tokens in the panel with one-time reveal, revoke, manual copy and late copy; tasks copy; viewers see the tasks alone");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
