// The admin graphs page is static: an HTML shell, uPlot and one script. Serve
// those files as they ship, answer /data with synthetic series, and drive the
// page like a person: draw the default panels, drag-zoom, double-click to
// reset and pick a range from the bar.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const assets = join(root, "crates/librepaper-server/src/server/graphs/assets");
const temp = mkdtempSync(join(tmpdir(), "librepaper-graphs-browser-"));
const files = {
  "/": ["index.html", "text/html"],
  "/app.js": ["app.js", "text/javascript"], "/uplot.js": ["uplot.iife.min.js", "text/javascript"],
  "/app.css": ["app.css", "text/css"], "/uplot.css": ["uplot.min.css", "text/css"],
};
// The twelve series in the order the server returns them.
const specs = [
  ["requests_per_minute", "count", "mean"], ["client_errors_per_minute", "count", "max"],
  ["server_errors_per_minute", "count", "max"], ["latency_p95_seconds", "seconds", "max"],
  ["documents_resident", "count", "mean"], ["sockets_active", "count", "mean"],
  ["storage_bytes", "bytes", "mean"], ["process_rss_bytes", "bytes", "mean"],
  ["memory_available_bytes", "bytes", "mean"], ["disk_available_bytes", "bytes", "mean"],
  ["host_cpu_percent", "percent", "mean"], ["db_connections_in_use", "count", "mean"],
];
// Every /data query the page made, parsed, oldest first.
const requests = [];
let server, tab;
try {
  server = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/data") {
      const [from, to, points] = ["from", "to", "points"].map((name) => Number(url.searchParams.get(name)));
      requests.push({ from, to, points });
      // A gentle sine per series, with every seventh sample missing so gaps draw.
      const series = specs.map(([name, unit, bucket], k) => ({
        name, label: name.replaceAll("_", " "), unit, bucket,
        values: Array.from({ length: points }, (_, i) => (i % 7 === 6 ? null : (k + 1) * (10 + 3 * Math.sin(i / 25 + k)))),
      }));
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify({ from, to, step: (to - from) / points, series }));
      return;
    }
    const file = files[url.pathname];
    if (!file) { response.statusCode = 404; response.end(); return; }
    response.setHeader("content-type", file[1]);
    response.end(readFileSync(join(assets, file[0])));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = 31000 + Math.floor(Math.random() * 1000);
  tab = await browser("chromium", join(temp, "profile"), port);
  await tab.resize(1200, 900);
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);

  // uPlot draws one canvas per plot, so twelve panels need twelve canvases.
  await until("twelve panels with one canvas each", () => tab.evaluate("document.querySelectorAll('#grid .panel').length === 12 && document.querySelectorAll('#grid canvas').length === 12"));
  await until("first /data request", () => requests.length > 0);
  assert.equal(requests[0].points, 600, "the page asks for 600 points");
  assert.equal(requests[0].to - requests[0].from, 86400, "the default range is 24 hours");

  // A left-button drag across the first plot's overlay selects a span.
  const rect = await tab.evaluate("(() => { const r = document.querySelector('#grid .panel .u-over').getBoundingClientRect(); return { x: r.left, y: r.top + r.height / 2, width: r.width }; })()");
  const x0 = rect.x + rect.width * 0.3, x1 = rect.x + rect.width * 0.6;
  const mouse = (type, x, extra = {}) => tab.command("Input.dispatchMouseEvent", { type, x, y: rect.y, button: "none", buttons: 0, ...extra });
  const held = { button: "left", buttons: 1 };
  let seen = requests.length;
  const previous = requests.at(-1);
  await mouse("mouseMoved", x0);
  await mouse("mousePressed", x0, { ...held, clickCount: 1 });
  for (let step = 1; step <= 8; step++) await mouse("mouseMoved", x0 + ((x1 - x0) * step) / 8, held);
  await mouse("mouseReleased", x1, { button: "left", clickCount: 1 });
  await until("narrower /data request after the drag", () => requests.slice(seen).find((one) => one.to - one.from < 86400 && one.from >= previous.from));

  // A double-click returns to the range bar's choice. Chromium turns the
  // second press and release into a dblclick; if it does not, send the event.
  seen = requests.length;
  const back = () => requests.slice(seen).find((one) => one.to - one.from === 86400);
  const xm = rect.x + rect.width / 2;
  for (const clickCount of [1, 2]) {
    await mouse("mousePressed", xm, { ...held, clickCount });
    await mouse("mouseReleased", xm, { button: "left", clickCount });
  }
  await until("24 hour /data request after the double-click", back, 3000).catch(async () => {
    await tab.evaluate("document.querySelector('#grid .panel .u-over').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))");
    return until("24 hour /data request after a dispatched dblclick", back, 10000);
  });

  // The range bar picks a fresh window.
  seen = requests.length;
  await tab.evaluate("document.querySelector('#ranges button[data-seconds=\"3600\"]').click()");
  await until("one hour /data request after the 1h button", () => requests.slice(seen).find((one) => one.to - one.from === 3600));

  // The page's CSP forbids inline styles, so nothing it made may carry one.
  assert.equal(await tab.evaluate("document.querySelectorAll('header [style], #ranges [style]').length"), 0, "header and range bar set no style attribute");
} finally {
  await tab?.close();
  await new Promise((resolve) => server ? server.close(resolve) : resolve());
  rmSync(temp, { recursive: true, force: true });
}
