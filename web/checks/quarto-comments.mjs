import assert from "node:assert/strict";
import { resultItems, resultAnchor, inspectResult } from "../src/lib/quarto-comments.js";
import { sha256 } from "../src/lib/quarto.js";

const text = "Previous statistical result";
const digest = await sha256(text);
const manifest = { render_id:"original", cells:[{ id:"chapters/paper.qmd#tbl-model", label:"tbl-model", outputs:[{ ordinal:0, kind:"text", text, content_sha256:digest }] }], assets:[] };
const item = resultItems(manifest)[0];
const anchor = resultAnchor(manifest,item);
assert.equal(anchor.cell_id,"chapters/paper.qmd#tbl-model");
assert.equal(anchor.render_id,"original");
const api = { quartoBundle:async (render) => { assert.equal(render,"original"); return { ok:true, json:async () => manifest }; } };
const inspected = await inspectResult(api,anchor);
assert.equal(inspected.items[0].output.text,text);
inspected.dispose();
await assert.rejects(inspectResult(api,{ ...anchor, content_sha256:"a".repeat(64) }),/does not match/);
await assert.rejects(inspectResult({ quartoBundle:async () => ({ ok:false }) },anchor),/unavailable/);
await assert.rejects(inspectResult({ quartoBundle:async () => ({ ok:true, json:async () => ({ ...manifest,render_id:"replacement" }) }) },anchor),/different render/);
const tampered = structuredClone(manifest);
tampered.cells[0].outputs[0].text = "Changed result";
await assert.rejects(inspectResult({ quartoBundle:async () => ({ ok:true,json:async () => tampered }) },anchor),/integrity/);
const image = { render_id:"plot", cells:[{ id:"paper.qmd#fig-main", outputs:[{ordinal:0,kind:"image",asset:"plot.png"}] }],assets:[{path:"plot.png",sha256:digest}] };
assert.equal(resultItems(image)[0].digest,digest,"asset identity supports collectors without a redundant output digest");
assert.deepEqual(resultAnchor(image,resultItems(image)[0],{width:800,height:600}),{
  render_id:"plot",cell_id:"paper.qmd#fig-main",output_ordinal:0,content_sha256:digest,coordinate_system:"percent",width:800,height:600,
});
const mismatchedImage = { ...image, cells:[{ ...image.cells[0], outputs:[{ ...image.cells[0].outputs[0], content_sha256:"b".repeat(64) }] }] };
await assert.rejects(inspectResult({ quartoBundle:async () => ({ ok:true, json:async () => mismatchedImage }) }, resultAnchor(mismatchedImage, resultItems(mismatchedImage)[0])), /image failed its integrity/);
console.log("quarto comments: immutable result lookup, integrity, asset identities, and region dimensions passed");
