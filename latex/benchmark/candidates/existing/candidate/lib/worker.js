import { create } from "./latex/texlyre.js";
let engine;
self.onmessage = async ({ data }) => {
  try {
    if (data.cmd === "choose") { engine = await create({ base: data.base, distribution: data.distribution }); self.postMessage({ ready: true }); }
    if (data.cmd === "compile") { const result = await engine.compile(data.tree); self.postMessage({ result }); }
  } catch (error) { self.postMessage({ error: String(error) }); }
};
