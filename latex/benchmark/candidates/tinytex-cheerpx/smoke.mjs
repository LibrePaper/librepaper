import { createServer } from "node:http";
import { readFileSync, statSync, openSync, readSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { browser, until } from "../../../../web/tools/browser-driver.mjs";
const root = dirname(fileURLToPath(import.meta.url));
const disk = join(root, "assets/rootfs.ext2"), size = statSync(disk).size, fd = openSync(disk, "r");
const server = createServer((req, res) => {
  const path = new URL(req.url, "http://localhost").pathname;
  res.setHeader("Cross-Origin-Opener-Policy", "same-origin"); res.setHeader("Cross-Origin-Embedder-Policy", "require-corp");
  if (path === "/assets/rootfs.ext2") { res.setHeader("Accept-Ranges","bytes"); res.setHeader("Content-Length",size); res.setHeader("Last-Modified",statSync(disk).mtime.toUTCString()); const m = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range || ""); if (!m) return res.end(); const start=+m[1], end=Math.min(+(m[2]||size-1),size-1); const body=Buffer.alloc(end-start+1); readSync(fd,body,0,body.length,start); res.statusCode=206; res.setHeader("Content-Range",`bytes ${start}-${end}/${size}`); res.setHeader("Content-Length",body.length); res.end(body); return; }
  if (path === "/smoke-input.json") {
    const ref = join(root, "../hybrid-validation/references/first");
    res.setHeader("Content-Type", "application/json");
    const expected=readFileSync(join(ref,"91-sorting-schemes.bbl"));
    return res.end(JSON.stringify({ bcf:[...readFileSync(join(ref,"91-sorting-schemes.bcf"))], bib:[...readFileSync(join(ref,"biblatex-examples.bib"))], bblSha256:createHash("sha256").update(expected).digest("hex") }));
  }
  const file = path === "/" ? "smoke.html" : path === "/biber-bridge.js" ? "biber-bridge.js" : path.slice(1);
  try { if (file.endsWith(".js")) res.setHeader("Content-Type","text/javascript"); res.end(readFileSync(join(root,file))); } catch { res.statusCode=404; res.end(); }
});
server.listen(8707, "127.0.0.1");
let driver;
try {
  rmSync(join(root,"profiles/smoke"),{recursive:true,force:true});
  driver = await browser("chromium", join(root,"profiles/smoke"), 9707); await driver.navigate("http://127.0.0.1:8707/");
  await until("bridge",async()=>{ const s=await driver.evaluate("({ready:globalThis.ready,error:globalThis.error})"); if(s.error) throw Error(s.error); return s.ready===true; },20000);
  await driver.evaluate("globalThis.smoke().then(r=>globalThis.result=r).catch(e=>globalThis.result={ok:false,error:String(e)});true");
  let result;
  await until("Biber smoke",async()=>{result=await driver.evaluate("globalThis.result||null");return result!==null},180000);
  writeFileSync(join(root,"results/bridge-smoke.json"),JSON.stringify(result,null,2)+"\n");
  console.log(JSON.stringify({...result,serial:result.serial?.slice(-2000)},null,2));
  if (!result.ok) process.exitCode=1;
} finally { await driver?.close();server.close(); }
