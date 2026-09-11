import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resultItems, resultAnchor, inspectResult } from "../src/lib/results-comments.js";
import { sha256 } from "../src/lib/results-hash.js";

const collaboration = await readFile(new URL("../src/components/Collaboration.svelte", import.meta.url), "utf8");
const comments = await readFile(new URL("../src/components/Comments.svelte", import.meta.url), "utf8");
const commentCard = await readFile(new URL("../src/components/CommentCard.svelte", import.meta.url), "utf8");
const reader = await readFile(new URL("../src/components/Reader.svelte", import.meta.url), "utf8");
assert.match(collaboration, /oninspectresult/);
assert.match(collaboration, /<Comments \{\.\.\.common\(\)\} \{comments\} filter="comments"/);
assert.match(comments, /oninspectresult/);
assert.match(comments, /Boolean\(comment\.output_anchor\)/);
assert.match(commentCard, /oninspectresult \? oninspectresult\(comment\)/);
// The reader no longer wires an inspector: nothing rendered is ever
// uploaded, so a saved-results panel to inspect no longer exists, and a
// comment with an `output_anchor` renders as an ordinary card (the prop
// stays supported for whoever else might use it).
assert.doesNotMatch(reader, /oninspectresult/);

const text = "Previous statistical result";
const digest = await sha256(text);
const manifest = { render_id:"original", cells:[{ id:"chapters/paper.qmd#tbl-model", label:"tbl-model", outputs:[{ ordinal:0, kind:"text", text, content_sha256:digest }] }], assets:[] };
const item = resultItems(manifest)[0];
const anchor = resultAnchor(manifest,item);
assert.equal(anchor.cell_id,"chapters/paper.qmd#tbl-model");
assert.equal(anchor.render_id,"original");
const api = { resultBundle:async (render) => { assert.equal(render,"original"); return { ok:true, json:async () => manifest }; } };
const inspected = await inspectResult(api,anchor);
assert.equal(inspected.items[0].output.text,text);
inspected.dispose();
await assert.rejects(inspectResult(api,{ ...anchor, content_sha256:"a".repeat(64) }),/does not match/);
await assert.rejects(inspectResult({ resultBundle:async () => ({ ok:false }) },anchor),/unavailable/);
await assert.rejects(inspectResult({ resultBundle:async () => ({ ok:true, json:async () => ({ ...manifest,render_id:"replacement" }) }) },anchor),/different render/);
const tampered = structuredClone(manifest);
tampered.cells[0].outputs[0].text = "Changed result";
await assert.rejects(inspectResult({ resultBundle:async () => ({ ok:true,json:async () => tampered }) },anchor),/integrity/);
const image = { render_id:"plot", cells:[{ id:"paper.qmd#fig-main", outputs:[{ordinal:0,kind:"image",asset:"plot.png"}] }],assets:[{path:"plot.png",sha256:digest}] };
assert.equal(resultItems(image)[0].digest,digest,"asset identity supports collectors without a redundant output digest");
assert.deepEqual(resultAnchor(image,resultItems(image)[0],{width:800,height:600}),{
  render_id:"plot",cell_id:"paper.qmd#fig-main",output_ordinal:0,content_sha256:digest,coordinate_system:"percent",width:800,height:600,
});
const mismatchedImage = { ...image, cells:[{ ...image.cells[0], outputs:[{ ...image.cells[0].outputs[0], content_sha256:"b".repeat(64) }] }] };
await assert.rejects(inspectResult({ resultBundle:async () => ({ ok:true, json:async () => mismatchedImage }) }, resultAnchor(mismatchedImage, resultItems(mismatchedImage)[0])), /image failed its integrity/);
console.log("quarto comments: immutable result lookup, integrity, asset identities, and region dimensions passed");
