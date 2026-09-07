// Inspect only this experiment's Chromium target while a long guest command runs.
const pages = await (await fetch('http://127.0.0.1:9705/json/list')).json();
const page = pages.find(p => p.url.startsWith('http://127.0.0.1:8705/'));
if (!page) throw new Error('No prototype page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
const pending = new Map(); let next = 1;
ws.onmessage = ({ data }) => { const m = JSON.parse(data); pending.get(m.id)?.(m); };
const evaluate = expression => new Promise(resolve => { const id = next++; pending.set(id, resolve); ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, returnByValue: true } })); });
try {
  const path = process.argv[2];
  if (path) {
    await evaluate(`vmSend(${JSON.stringify({ type: 'read', id: 'inspection', path })})`);
    await new Promise(resolve => setTimeout(resolve, 500));
    const m = await evaluate('vm.files.inspection');
    const result = m.result?.result?.value;
    console.log(result?.bytes ? Buffer.from(result.bytes, 'base64').toString('utf8').slice(-5000) : result);
  } else {
    const m = await evaluate('({status:vm.status,activity:vm.activity,error:vm.error,serial:vm.serial.slice(-4000),commands:vm.commands,files: Object.fromEntries(Object.entries(vm.files).map(([k,v])=>[k,{written:v.written,error:v.error}]))})');
    console.log(JSON.stringify(m.result?.result?.value, null, 2));
  }
} finally { ws.close(); }
