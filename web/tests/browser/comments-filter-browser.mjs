import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

// What the filter over the comments must not cost the person using it.
//
// A card that leaves the list takes its component with it, and three things
// used to go the same way: the words of a draft whose slot depended on that
// card, an unsent reply typed into it, and the keyboard's focus when the
// button that was pressed stopped existing. A draft keeps its words wherever
// its card is put; a card holding unsent words stays listed; and the focus
// goes to the check the card now has, or to the card that took its place.
// The filter's choices are radio items, and an empty tab says it is empty.
const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-comments-filter-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { createClassComponent } from ${JSON.stringify(join(root, "node_modules/svelte/src/legacy/legacy-client.js"))};
import Comments from ${JSON.stringify(join(root, "src/components/Comments.svelte"))};
const settle=()=>new Promise(resolve=>setTimeout(resolve,150));
window.note=(id,seq,start,over={})=>({id,seq,start,motivation:'commenting',presentation:{rendered_exact:'Passage '+id},body:'Body of '+id,deletable:true,replies:[],reply_total:0,...over});
window.pageOf=(total,open)=>({loading:false,total,open,replies:0,revision:'r',cursor:null,complete:true,pages:1,error:'',busy:false});
let comments=[];let panel=null;let host=null;
// Resolving is answered the way the room answers it: the comment comes back
// with its state flipped, and the panel is handed the new list.
const flip=id=>{comments=comments.map(one=>one.id===id?{...one,resolved:!one.resolved}:one);panel.$set({comments});};
window.mount=async(list,props={})=>{
  if(panel){panel.$destroy();host.remove();}
  comments=list;
  host=document.createElement('div');
  host.style.cssText='display:flex;flex-direction:column;width:360px;height:600px;padding:16px';
  document.body.append(host);
  panel=createClassComponent({component:Comments,target:host,props:{comments,canModerate:true,onresolve:c=>flip(c.id),...props}});
  await settle();
};
// Somebody else resolves or reopens a thread.
window.remote=async id=>{flip(id);await settle();};
window.cards=()=>Array.from(document.querySelectorAll(".comments-list article")).map(node=>node.id);
window.text=()=>document.querySelector(".comments-panel").innerText;
const press=node=>{for(const type of ['pointerdown','pointerup','click'])node.dispatchEvent(new PointerEvent(type,{bubbles:true,composed:true,button:0,isPrimary:true,pointerType:'mouse'}));};
const openItems=()=>Array.from(document.querySelectorAll('[data-part="content"][data-state="open"] [data-part="item"]'));
const label=node=>node.textContent.replace("\\u2713","").trim();
window.filter=async name=>{
  const trigger=document.querySelector(".filter-trigger");
  if(!trigger)return false;
  press(trigger);await settle();
  const item=openItems().find(node=>label(node)===name);
  if(!item){press(trigger);await settle();return false;}
  press(item);await settle();
  return true;
};
window.choices=async()=>{
  const trigger=document.querySelector(".filter-trigger");
  press(trigger);await settle();
  const found=openItems().map(node=>[node.getAttribute("role"),node.getAttribute("aria-checked"),label(node)]);
  press(trigger);await settle();
  return found;
};
window.type=async(selector,words)=>{
  const field=document.querySelector(selector);
  if(!field)return false;
  field.focus();field.value=words;field.dispatchEvent(new Event('input',{bubbles:true}));
  await settle();
  return true;
};
window.value=selector=>document.querySelector(selector)?.value??null;
// The keyboard's way to a verb: the button is focused, then activated.
window.verb=async(card,name)=>{
  const button=document.querySelector('#comment-'+card+' button[aria-label="'+name+'"]');
  if(!button)return false;
  button.focus();button.click();
  await settle();
  return true;
};
// Where the keyboard is: the card it is in, and the name of the control.
window.focused=()=>{
  const node=document.activeElement;
  if(!node||node===document.body)return "nowhere";
  return (node.closest("article")?.id??"")+"|"+(node.getAttribute("aria-label")??node.tagName.toLowerCase());
};
window.ready=true;
`);
let server, page;
try {
  await build({ configFile: false, root, plugins: [svelte(), tailwindcss()], logLevel: "error",
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } } });
  server = createServer((request, response) => {
    const file = request.url === "/panel.js" ? "panel.js" : request.url === "/style.css" ? "librepaper-web.css" : null;
    response.setHeader("content-type", file?.endsWith("js") ? "text/javascript" : file ? "text/css" : "text/html; charset=utf-8");
    response.end(file ? readFileSync(join(temporary, "build", file)) : '<!doctype html><html data-theme="librepaper"><head><meta charset="utf-8"><link rel="stylesheet" href="/style.css"></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("the harness", () => page.evaluate("window.ready === true"), 20000);
  const three = "[window.note('a',0,10), window.note('done',1,20,{resolved:true}), window.note('b',2,30)]";

  // The choices are one of three, and say which one to a screen reader.
  await page.evaluate(`window.mount(${three})`);
  assert.deepEqual(await page.evaluate("window.choices()"), [
    ["menuitemradio", "true", "Unresolved"],
    ["menuitemradio", "false", "Resolved"],
    ["menuitemradio", "false", "All"],
  ]);

  // A draft keeps its words when the card it stood before leaves or arrives.
  // Its passage sits between the first comment and the resolved one, so the
  // resolved card coming into the list moves the draft to a new slot.
  await page.evaluate(`window.mount(${three}, {composing:{id:'d1',pending:{exact:'a new passage',position:15}}})`);
  await until("the composer", () => page.evaluate('Boolean(document.querySelector("#composer textarea"))'), 5000);
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-a", "composer", "comment-b"]);
  assert.ok(await page.evaluate(`window.type("#composer textarea", "Half a thought")`));
  assert.ok(await page.evaluate('window.filter("All")'));
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-a", "composer", "comment-done", "comment-b"]);
  assert.equal(await page.evaluate('window.value("#composer textarea")'), "Half a thought",
    "the draft kept its words when its slot moved");
  assert.ok(await page.evaluate('window.filter("Unresolved")'));
  assert.equal(await page.evaluate('window.value("#composer textarea")'), "Half a thought");

  // A card holding an unsent reply stays listed whatever the filter says, and
  // leaves once there is nothing left to lose.
  await page.evaluate(`window.mount(${three})`);
  assert.ok(await page.evaluate('window.verb("a", "Reply")'));
  assert.ok(await page.evaluate(`window.type("#comment-a textarea", "Not sent yet")`));
  assert.ok(await page.evaluate('window.filter("Resolved")'));
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-a", "comment-done"]);
  assert.equal(await page.evaluate('window.value("#comment-a textarea")'), "Not sent yet");
  assert.ok(await page.evaluate(`window.type("#comment-a textarea", "")`));
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-done"]);

  // Somebody else resolves the thread while a reply to it is being typed: the
  // card neither leaves the list nor folds away under the writer.
  await page.evaluate(`window.mount(${three})`);
  assert.ok(await page.evaluate('window.verb("a", "Reply")'));
  assert.ok(await page.evaluate(`window.type("#comment-a textarea", "Still typing")`));
  await page.evaluate('window.remote("a")');
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-a", "comment-b"]);
  assert.equal(await page.evaluate('window.value("#comment-a textarea")'), "Still typing",
    "the reply box is still there, with its words");

  // Resolving and reopening from the keyboard replace the button that was
  // pressed. The focus goes to the check the card now has.
  await page.evaluate(`window.mount(${three})`);
  assert.ok(await page.evaluate('window.filter("All")'));
  assert.ok(await page.evaluate('window.verb("a", "Resolve")'));
  assert.equal(await page.evaluate("window.focused()"), "comment-a|Reopen");
  assert.ok(await page.evaluate('window.verb("a", "Reopen")'));
  assert.equal(await page.evaluate("window.focused()"), "comment-a|Resolve");

  // Under a filter the card leaves altogether. The card that took its place
  // has the focus, and the filter has it once the list is empty.
  assert.ok(await page.evaluate('window.filter("Unresolved")'));
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-a", "comment-b"]);
  assert.ok(await page.evaluate('window.verb("a", "Resolve")'));
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-b"]);
  assert.match(await page.evaluate("window.focused()"), /^comment-b\|/,
    "the next card takes the focus of the one that left");
  assert.ok(await page.evaluate('window.verb("b", "Resolve")'));
  assert.deepEqual(await page.evaluate("window.cards()"), []);
  assert.equal(await page.evaluate("window.focused()"), "|Which comments to show: Unresolved");
  assert.ok(await page.evaluate('window.text().includes("Nothing matches this filter.")'));

  // An empty tab says so in its own words, also when the document has
  // annotations of the other kind.
  await page.evaluate(`window.mount([window.note('a',0,10)], {filter:'highlights', empty:'No highlights yet.', page:window.pageOf(1,1)})`);
  assert.ok(await page.evaluate('window.text().includes("No highlights yet.")'));
  await page.evaluate(`window.mount([], {page:window.pageOf(0,0)})`);
  assert.ok(await page.evaluate('window.text().includes("No comments yet.")'));

  console.log("comments-filter-browser: radio choices; a draft, an unsent reply and the keyboard's focus all survive a card leaving the list; empty tabs say so");
} finally {
  await page?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
