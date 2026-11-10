// The preview's in-place morph, run in a real browser: unchanged nodes must
// keep their identity (no image blink), and the target must always end up
// identical to the source, whatever moved, was renumbered, added or removed.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const morphSource = readFileSync(fileURLToPath(new URL("../../src/agent/morph.js", import.meta.url)), "utf8");
const temporary = mkdtempSync(join(tmpdir(), "librepaper-morph-"));
const page_html = `<!doctype html><body><script type="module">
import { morphChildren } from "/morph.js";
const para = (n, text, id) => '<p id="S1.p' + id + '" data-sourcepos="1:' + n + '">' + text + ' <a href="#S1.p' + id + '">ref</a></p>';
const image = '<img id="S1.F1" src="data:image/gif;base64,R0lGODlhAQABAAAAACw=">';
const build = (paras) => paras.map(([n, text], i) => para(n, text, i + 1)).join("") ;
const base = [[1, "alpha"], [2, "beta"], [3, "gamma"], [4, "delta"], [5, "epsilon"]];
const withImage = (paras) => build(paras.slice(0, 2)) + image + build(paras.slice(2).map((p, i) => p)).replace(/S1\\.p(\\d+)/g, (m, d) => "S1.p" + (Number(d) + 2));
// Run one morph from a fresh target and report on identity and mutations.
window.run = (before, after, tag) => {
  const target = document.createElement("div");
  target.innerHTML = before;
  document.body.append(target);
  const source = new DOMParser().parseFromString("<body>" + after + "</body>", "text/html").body;
  const tagged = [...target.children];
  tagged.forEach((node, i) => { node.expando = i; });
  const records = [];
  const observer = new MutationObserver(() => {});
  observer.observe(target, { subtree: true, attributes: true, characterData: true, childList: true });
  morphChildren(target, source);
  records.push(...observer.takeRecords());
  observer.disconnect();
  const result = {
    equal: target.innerHTML === source.innerHTML,
    mutations: records.length,
    types: [...new Set(records.map((r) => r.type))],
    survivors: [...target.children].map((node) => node.expando),
    imgSame: target.querySelector("img")?.expando === tagged.findIndex((n) => n.localName === "img"),
  };
  target.remove();
  return result;
};
window.inputs = { base, para, image, build };
window.ready = true;
</script>`;

let server, page;
try {
  server = createServer((request, response) => {
    if (request.url === "/morph.js") {
      response.setHeader("content-type", "text/javascript; charset=utf-8");
      response.end(morphSource);
    } else {
      response.setHeader("content-type", "text/html; charset=utf-8");
      response.end(page_html);
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  page = await browser(process.env.BROWSER || "chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("morph page ready", () => page.evaluate("window.ready === true"), 10000);

  // Documents are built page-side from small helpers; each case is an
  // expression that returns the run() report.
  const doc = (texts, ids, withImg = true) => `(() => {
    const { para, image } = window.inputs;
    const texts = ${JSON.stringify(texts)}, ids = ${JSON.stringify(ids)};
    return texts.map((t, i) => para(i + 1, t, ids[i]) + (withImg && i === 1 ? image : "")).join("");
  })()`;
  const run = (before, after) => page.evaluate(`window.run(${before}, ${after})`);
  const five = ["alpha", "beta", "gamma", "delta", "epsilon"];
  const ids = [1, 2, 3, 4, 5];

  let result = await run(doc(five, ids), doc(five, ids));
  assert.equal(result.equal, true);
  assert.equal(result.mutations, 0, "identical source makes no mutations");

  result = await run(doc(five, ids), doc(["alpha", "beta", "gamma changed", "delta", "epsilon"], ids));
  assert.equal(result.equal, true);
  assert.deepEqual(result.survivors, [0, 1, 2, 3, 4, 5], "every node survives a word change (image included)");
  assert.deepEqual(result.types, ["characterData"], "only character data mutates");
  assert.equal(result.imgSame, true);

  result = await run(doc(five, ids), doc(["new", ...five], [1, 2, 3, 4, 5, 6]));
  assert.equal(result.equal, true);
  assert.equal(result.imgSame, true, "the image is never recreated on insertion with renumbered ids");
  assert.equal(result.survivors.filter((s) => s !== undefined).length, 6, "old nodes are reused despite renumbering");

  result = await run(doc(five, ids), doc(["alpha", "beta", "delta", "epsilon"], ids));
  assert.equal(result.equal, true);
  assert.equal(result.imgSame, true);
  assert.equal(result.survivors.filter((s) => s !== undefined).length, 5, "deletion keeps the rest");

  result = await run(doc(five, ids), doc(["alpha", "delta", "beta", "gamma", "epsilon"], ids));
  assert.equal(result.equal, true);
  assert.equal(result.imgSame, true);
  assert.equal(result.survivors.filter((s) => s !== undefined).length, 6, "reordering moves nodes rather than recreating them");

  console.log("morph-browser: identical source, word change, renumbering insert, delete and reorder passed");
} finally {
  await page?.close?.();
  if (server) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
