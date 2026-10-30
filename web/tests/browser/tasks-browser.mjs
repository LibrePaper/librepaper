// A browser-level check for the task controls of the Markdown visual layer: the
// task boxes, which cycle a task's state on each click, and the quiet field and
// tag chips, which are drawn as spans and never open a popup. Each action is
// checked against the shared document, read from the Loro text, so every assertion
// is about what the editor writes. The Editor is mounted with the layer on over a
// Loro-backed session.

import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { pause, removeTemporary, stopBrowserProcess, until } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-tasks-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);

// The task list under test. The caret is parked on "End.", the last line, so that
// no task line is drawn as source. The top level has five tasks and the parent has
// two children, so eight task boxes are drawn.
const sample = [
  "# Tasks",
  "",
  "- [ ] Write intro priority: 2 due: 2026-11-05 #paper",
  "- [/] Analyze results",
  "- [x] Submit abstract",
  "- [ ] Bad priority priority:9",
  "- [ ] Parent",
  "  - [x] child one",
  "  - [ ] child two",
  "",
  "End.",
].join("\n");

const BOX = ".cm-lp-taskbox-button";

// Runs in the page. The shared text of tasks.md, read from the Loro document.
function tasksText() {
  const { session, file } = window.tasksHarness;
  return session.doc.getMap("files").get(file).toString();
}

// Runs in the page. Moves the caret to "End.", so no task line is drawn as source.
function tasksPark() {
  const view = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  view.dispatch({ selection: { anchor: view.state.doc.toString().indexOf("End.") } });
}

// Runs in the page. The first drawn line whose text holds every word, or null.
function tasksRow(words) {
  const lines = Array.from(document.querySelectorAll(".cm-editor .cm-line"));
  return lines.find((line) => words.every((word) => line.textContent.includes(word))) ?? null;
}

// Runs in the page. Reads the first control matching the selector in the row with the words.
// Returns its tag, text, its aria-checked value and its class names, or null when it is missing.
function tasksRead(words, selector) {
  const control = tasksRow(words)?.querySelector(selector);
  if (!control) return null;
  return {
    tag: control.tagName,
    text: control.textContent.trim(),
    checked: control.getAttribute("aria-checked"),
    classes: control.className,
  };
}

// Runs in the page. Clicks the first control matching the selector in the row with the words.
// Returns whether a control was clicked.
function tasksClick(words, selector) {
  const control = tasksRow(words)?.querySelector(selector);
  if (!control) return false;
  control.click();
  return true;
}

const entrySource = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};`,
  `import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};`,
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `document.documentElement.dataset.theme = "librepaper";`,
  `document.documentElement.dataset.palette = "librepaper";`,
  `document.documentElement.dataset.mode = "light";`,
  `const host = document.createElement("main");`,
  `host.style.cssText = "height: 100vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(host);`,
  `const session = joinSession({ send: () => {}, mayEdit: true });`,
  `const file = session.addText("tasks.md", ${JSON.stringify(sample)});`,
  `window.tasksHarness = { session, file };`,
  `createClassComponent({`,
  `  component: Editor,`,
  `  target: host,`,
  `  props: { session, format: "markdown", file, visual: true },`,
  `});`,
  tasksText.toString(),
  tasksPark.toString(),
  tasksRow.toString(),
  tasksRead.toString(),
  tasksClick.toString(),
  `Object.assign(window, { tasksText, tasksPark, tasksRead, tasksClick });`,
  `window.tasksReady = true;`,
  "",
].join("\n");

writeFileSync(entry, entrySource);
let server;
let browser;
let socket;
// Browser errors and uncaught exceptions, gathered over CDP while the check runs.
const errors = [];
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    define: { __KATEX__: JSON.stringify("/katex/") },
    resolve: { alias: loroAlias },
    plugins: [tailwindcss(), svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "tasks-check.js", cssFileName: "tasks-check" },
    },
  });

  server = createServer((request, response) => {
    const path = decodeURIComponent(request.url.split("?")[0]);
    const file = join(output, path.slice(1));
    if (path !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><body><link rel="stylesheet" href="/tasks-check.css"><script type="module" src="/tasks-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local tasks server did not start");

  browser = spawn(requireChromiumExecutable(), [
    "--headless=new",
    "--window-size=1280,900",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore", detached: true });
  let browserInfo;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      browserInfo = await fetch(`http://127.0.0.1:${port}/json`).then((answer) => answer.json());
      break;
    } catch {
      await pause(100);
    }
  }
  const page = browserInfo?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });

  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.method === "Runtime.exceptionThrown") {
      const details = message.params.exceptionDetails;
      errors.push(details.exception?.description ?? details.text);
    }
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") {
      errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "));
    }
    const resolve = pending.get(message.id);
    if (resolve) {
      pending.delete(message.id);
      resolve(message);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    const id = ++sequence;
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const message = await send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };
  const read = () => evaluate("tasksText()");
  const readControl = (words, selector) => evaluate(`tasksRead(${JSON.stringify(words)}, ${JSON.stringify(selector)})`);
  const clickControl = (words, selector) => evaluate(`tasksClick(${JSON.stringify(words)}, ${JSON.stringify(selector)})`);
  const checkedOf = async (words) => (await readControl(words, BOX))?.checked ?? null;
  const popupCount = () => evaluate("document.querySelectorAll('.cm-lp-task-popup').length");
  // Waits until the shared text is the expected text, then returns the text read last.
  const settle = async (expected) => {
    await until("the shared text", async () => (await read()) === expected, 5000).catch(() => {});
    return read();
  };

  await send("Runtime.enable");
  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  await until("the Editor", () => evaluate("Boolean(window.tasksReady && document.querySelector('.cm-editor'))"), 10000);
  await evaluate("tasksPark()");
  await until("the task boxes", async () => (await evaluate(`document.querySelectorAll("${BOX}").length`)) > 0, 10000);

  // Check 1: drawing changes nothing, and each control shows its task's state.
  assert.equal(await read(), sample, "drawing the task list changed the shared text");
  assert.equal(await evaluate(`document.querySelectorAll(".cm-editor ${BOX}").length`), 7, "the seven task boxes of the source were not all drawn");
  assert.equal(await checkedOf(["Write intro"]), "false", "the todo box does not read aria-checked false");
  assert.equal(await checkedOf(["Analyze results"]), "mixed", "the in-progress box does not read aria-checked mixed");
  assert.equal(await checkedOf(["Submit abstract"]), "true", "the done box does not read aria-checked true");
  assert.equal((await readControl(["Write intro"], ".cm-lp-field-priority"))?.tag, "SPAN", "the P2 chip is not drawn as a span");
  assert.equal((await readControl(["Write intro"], ".cm-lp-field-priority"))?.text, "P2", "the priority chip on the Write intro line does not read P2");
  assert.equal((await readControl(["Write intro"], ".cm-lp-field-due"))?.tag, "SPAN", "the due chip is not drawn as a span");
  assert.equal((await readControl(["Parent"], ".cm-lp-task-progress"))?.text, "1 of 2", "the parent does not show its progress 1 of 2");
  assert.notEqual(await readControl(["Bad priority"], ".cm-lp-task-invalid"), null, "the priority:9 chip does not have the class cm-lp-task-invalid");
  console.log("tasks-browser: the controls are drawn with the states, the progress and the warning, and the text is unchanged");

  // Check 2: each click on the first box moves its task to the next state. The fifth
  // click returns the task to to do, and the text is the original again.
  let expected = sample;
  const states = ["[ ]", "[x]", "[/]", "[?]", "[-]", "[ ]"];
  for (let index = 1; index < states.length; index++) {
    assert.equal(await clickControl(["Write intro"], BOX), true, `no task box on the Write intro line before ${states[index]}`);
    expected = expected.replace(`- ${states[index - 1]} Write intro`, `- ${states[index]} Write intro`);
    assert.equal(await settle(expected), expected, `a click on the Write intro box did not write ${states[index]}`);
  }
  assert.equal(expected, sample, "the cycle did not return the Write intro line to the original text");
  console.log("tasks-browser: each click on a box moves the task to the next state, and the cycle returns to to do");

  // Check 3: a click on the in-progress box writes [?], and a click on the done box writes [/].
  assert.equal(await clickControl(["Analyze results"], BOX), true, "no task box on the Analyze results line");
  expected = sample.replace("- [/] Analyze results", "- [?] Analyze results");
  assert.equal(await settle(expected), expected, "a click on the in-progress box did not write [?]");
  assert.equal(await clickControl(["Submit abstract"], BOX), true, "no task box on the Submit abstract line");
  expected = expected.replace("- [x] Submit abstract", "- [/] Submit abstract");
  assert.equal(await settle(expected), expected, "a click on the done box did not write [/]");
  console.log("tasks-browser: the in-progress box moves to blocked, and the done box moves to in progress");

  // Check 4: no click opens a popup, and the chips change no text.
  assert.equal(await clickControl(["Write intro"], ".cm-lp-field-priority"), true, "no priority chip on the Write intro line");
  assert.equal(await clickControl(["Write intro"], ".cm-lp-task-tag"), true, "no tag chip on the Write intro line");
  assert.equal(await popupCount(), 0, "a chip opened a popup");
  assert.equal(await read(), expected, "a click on a chip changed the shared text");
  console.log("tasks-browser: chip clicks open no popup and change no text");

  assert.equal(errors.length, 0, ["Browser errors or exceptions during the tasks check:", ...errors].join("\n"));
  console.log("tasks-browser: ok");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
