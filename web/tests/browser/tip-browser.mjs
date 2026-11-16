// The application's one tooltip (lib/tip.js), in a real browser: it opens on
// hover and on keyboard focus, closes on Escape and on a press, swaps along a
// row of buttons, waits longer where asked, stays alone, and shows above an
// open modal dialog.
import assert from "node:assert/strict";
import { build } from "vite";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, pause, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType } from "../helpers/loro.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-tip-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { installTips } from ${JSON.stringify(join(root, "web/src/lib/tip.js"))};
installTips();
installTips();
document.body.innerHTML = \`
  <div style="padding: 60px 40px">
    <button id="plain" data-tip="Save the file">Plain</button>
    <button id="keyed" data-tip="Bold" data-tip-key="Ctrl+B">Keyed</button>
    <span id="slow" tabindex="0" data-tip="Full clipped name" data-tip-wait="long">Slow</span>
    <button id="left" data-tip="Left button">L</button><button id="right" data-tip="Right button">R</button>
    <span id="whole" data-tip="All of it shows" data-tip-wait="long" style="display: inline-block">All of it shows</span>
    <span id="clipped" data-tip="A name too long for its box" data-tip-wait="long" style="display: inline-block; width: 60px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; vertical-align: bottom">A name too long for its box</span>
  </div>
  <div id="reference" class="card lp-control-neutral px-2 py-1 text-xs shadow-lg" style="display: inline-block">Reference</div>
  <dialog id="dialog"><button id="inside" data-tip="Inside the dialog">Inside</button></dialog>\`;
document.getElementById("dialog").showModal();
`);

let http, tab;
const rect = (selector) => tab.evaluate(`(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height }; })()`);
const centre = async (selector) => { const r = await rect(selector); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
const move = async (x, y) => tab.command("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
const hover = async (selector) => { const p = await centre(selector); await move(p.x, p.y); };
const tipText = () => tab.evaluate(`document.querySelector("[role=tooltip] .tip-text")?.textContent`);
const visible = () => tab.evaluate(`(() => { const tip = document.querySelector("[role=tooltip]"); return !!tip && getComputedStyle(tip).display !== "none" && (!tip.matches("[popover]") || tip.matches(":popover-open")); })()`);
const key = (type, name, code, vk) => tab.command("Input.dispatchKeyEvent", { type, key: name, code, windowsVirtualKeyCode: vk });

try {
  await build({ configFile: false, root: join(root, "web"), plugins: [tailwindcss()], build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" } }, logLevel: "error" });
  http = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/") {
      response.setHeader("content-type", "text/html");
      return response.end('<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/check.css"></head><body><script type="module" src="/check.js"></script></body></html>');
    }
    try {
      const file = join(out, url.pathname.slice(1));
      response.setHeader("content-type", contentType(file)); response.end(readFileSync(file));
    } catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve, reject) => { http.once("error", reject); http.listen(0, "127.0.0.1", resolve); });
  const origin = `http://127.0.0.1:${http.address().port}`;
  tab = await browser("chromium", join(temp, "profile"), 37000 + Math.floor(Math.random() * 1000));
  await tab.resize(1000, 700);
  await tab.navigate(`${origin}/`);
  await until("harness mounted", () => tab.evaluate(`!!document.getElementById("plain")`), 20000);

  // The modal dialog makes the page behind it inert for the pointer, so the
  // page's own triggers are tested with it closed, and it is opened again for
  // the last check.
  await tab.evaluate(`document.getElementById("dialog").close()`);

  assert.equal(await visible(), false, "no tip before hovering");

  await hover("#plain");
  await until("tip shown on hover", visible, 3000);
  assert.equal(await tipText(), "Save the file");
  assert.equal(await tab.evaluate(`document.querySelectorAll("[role=tooltip]").length`), 1, "exactly one tooltip element");
  assert.equal(await tab.evaluate(`getComputedStyle(document.querySelector("[role=tooltip] .tip-key")).display`), "none", "no keycap without a key");
  const trigger = await rect("#plain"), box = await rect("[role=tooltip]");
  assert.ok(box.top >= trigger.bottom, `tip sits below the trigger (${box.top} < ${trigger.bottom})`);
  assert.ok(box.left >= 0 && box.right <= 1000 && box.bottom <= 700, "tip is inside the viewport");
  assert.equal(await tab.evaluate(`document.getElementById("plain").getAttribute("aria-describedby")`), "lp-tip", "tip described the trigger while open");
  // The tip is a popover, and the browser's popover defaults (a border, a
  // padding, system colours) must not show through the card it is drawn as.
  const look = (selector) => tab.evaluate(`(() => { const s = getComputedStyle(document.querySelector(${JSON.stringify(selector)})); return ["borderTopWidth", "borderTopStyle", "borderTopColor", "paddingTop", "paddingLeft", "backgroundColor", "color", "fontSize", "lineHeight", "borderTopLeftRadius", "boxShadow"].map((name) => name + ": " + s[name]).join("; "); })()`);
  assert.equal(await look("[role=tooltip]"), await look("#reference"), "the tip looks like the card it is");

  await key("keyDown", "Escape", "Escape", 27); await key("keyUp", "Escape", "Escape", 27);
  await until("Escape hides the tip", async () => !(await visible()), 2000);
  assert.equal(await tab.evaluate(`document.getElementById("plain").hasAttribute("aria-describedby")`), false, "description removed on close");

  await move(5, 5);
  await pause(400);
  await hover("#plain");
  await until("tip shown again", visible, 3000);
  const p = await centre("#plain");
  await tab.command("Input.dispatchMouseEvent", { type: "mousePressed", x: p.x, y: p.y, button: "left", clickCount: 1 });
  await until("a press hides the tip", async () => !(await visible()), 2000);
  await tab.command("Input.dispatchMouseEvent", { type: "mouseReleased", x: p.x, y: p.y, button: "left", clickCount: 1 });

  await move(5, 5);
  await pause(400);
  await hover("#keyed");
  await until("keyed tip shown", visible, 3000);
  assert.equal(await tipText(), "Bold");
  assert.equal(await tab.evaluate(`document.querySelector("[role=tooltip] .tip-key").textContent`), "Ctrl+B");
  assert.notEqual(await tab.evaluate(`getComputedStyle(document.querySelector("[role=tooltip] .tip-key")).display`), "none", "keycap is drawn");

  // Along a row the text swaps without the tip closing.
  await hover("#left");
  await until("left button's tip", async () => (await tipText()) === "Left button", 1000);
  await hover("#right");
  await until("right button's tip", async () => (await tipText()) === "Right button", 1000);
  assert.equal(await tab.evaluate(`document.querySelectorAll("[role=tooltip]").length`), 1, "still one tooltip element");

  await move(5, 5);
  await until("tip closes after the pointer leaves", async () => !(await visible()), 2000);

  await hover("#slow");
  await pause(300);
  assert.equal(await visible(), false, "a long-wait tip is not shown at 300 ms");
  await until("a long-wait tip is shown by 1200 ms", visible, 900);
  assert.equal(await tipText(), "Full clipped name");

  await move(5, 5);
  await until("long tip closes", async () => !(await visible()), 2000);

  // A slow tip waits even while another tip is open, and that one closes.
  await hover("#plain");
  await until("the plain tip again", async () => (await tipText()) === "Save the file" && (await visible()), 3000);
  await hover("#slow");
  await pause(300);
  assert.equal(await visible(), false, "the open tip closed and the slow one is still waiting");
  await until("the slow tip after its wait", async () => (await visible()) && (await tipText()) === "Full clipped name", 900);
  await move(5, 5);
  await until("slow tip closes", async () => !(await visible()), 2000);

  // A tip that repeats text shown in full stays away; cut off, it shows.
  await pause(400);
  await hover("#whole");
  await pause(900);
  assert.equal(await visible(), false, "no tip for text that already shows in full");
  await hover("#clipped");
  await until("a tip for the clipped text", async () => (await visible()) && (await tipText()) === "A name too long for its box", 2000);
  await move(5, 5);
  await until("clipped tip closes", async () => !(await visible()), 2000);

  // Keyboard focus shows the tip of whichever control Tab lands on.
  await key("keyDown", "Tab", "Tab", 9); await key("keyUp", "Tab", "Tab", 9);
  await until("keyboard focus shows the tip", visible, 3000);
  assert.equal(await tipText(), await tab.evaluate(`document.activeElement.getAttribute("data-tip")`), "the tip is the focused control's");
  await tab.evaluate(`document.activeElement.blur()`);
  await until("blur hides the tip", async () => !(await visible()), 2000);

  // Above an open modal dialog.
  await tab.evaluate(`document.getElementById("dialog").showModal()`);
  await move(5, 5);
  await hover("#inside");
  await until("tip shown inside the dialog", visible, 3000);
  assert.equal(await tipText(), "Inside the dialog");
  const inside = await rect("#inside"), tipBox = await rect("[role=tooltip]");
  assert.ok(tipBox.width > 0 && tipBox.height > 0 && tipBox.top >= inside.bottom, "tip is laid out below its trigger in the dialog");
  // The dialog is in the top layer too, so the tip is above it only if it joined the top layer later.
  assert.equal(await tab.evaluate(`document.querySelector("[role=tooltip]").matches(":popover-open")`), true, "tip is in the top layer");
  console.log("tip: hover, focus, keycap, swap, long wait, repeated text, Escape, press and dialog passed");
} catch (error) {
  if (tab) console.error(await tab.evaluate(`document.querySelector("[role=tooltip]")?.outerHTML`).catch(() => ""));
  throw error;
} finally {
  await tab?.close(); http?.close(); removeTemporary(temp);
}
