import { createServer } from 'node:http';
import { readFileSync, statSync, openSync, readSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('.',import.meta.url));
const disk=root+'assets/rootfs.ext2', size=statSync(disk).size, fd=openSync(disk,'r');
let counters={bytes:0,requests:0}; const lifetime={bytes:0,requests:0};
export const server=createServer((req,res)=>{
  const path=new URL(req.url,'http://localhost').pathname;
  res.setHeader('Cross-Origin-Opener-Policy','same-origin');
  res.setHeader('Cross-Origin-Embedder-Policy','require-corp');
  res.setHeader('Cache-Control','no-store');
  if(path==='/__reset') counters={bytes:0,requests:0};
  if(path.startsWith('/__')) {res.setHeader('Content-Type','application/json');return res.end(JSON.stringify({...counters,lifetime}));}
  let body;
  if(path==='/assets/rootfs.ext2') {
    res.setHeader('Last-Modified',statSync(disk).mtime.toUTCString());
    res.setHeader('Accept-Ranges','bytes');res.setHeader('Content-Type','application/octet-stream');
    if(req.method==='HEAD') {res.setHeader('Content-Length',size);return res.end();}
    const match=/^bytes=(\d+)-(\d*)$/.exec(req.headers.range||'');
    if(!match) {res.writeHead(400);return res.end('Range required');}
    const start=Number(match[1]),end=Math.min(Number(match[2]||size-1),size-1);
    if(start>end||end-start>16*1024*1024) {res.writeHead(416);return res.end();}
    body=Buffer.alloc(end-start+1);readSync(fd,body,0,body.length,start);
    res.statusCode=206;res.setHeader('Content-Range',`bytes ${start}-${end}/${size}`);
  } else if(['/','/adapter.js'].includes(path)) {
    body=readFileSync(root+(path==='/'?'index.html':'adapter.js'));
    res.setHeader('Content-Type',path==='/'?'text/html':'text/javascript');
  } else {res.writeHead(404);return res.end();}
  counters.bytes+=body.length;counters.requests++;lifetime.bytes+=body.length;lifetime.requests++;
  res.setHeader('Content-Length',body.length);res.end(body);
});
server.listen(8705,'127.0.0.1',()=>console.log('TinyTeX/CheerpX: http://127.0.0.1:8705'));
