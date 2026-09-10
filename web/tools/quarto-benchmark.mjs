// Reproducible draft benchmark; execution, network, and browser paint are separate.
import { performance } from "node:perf_hooks";
import { parseQuarto, composeDraft } from "../src/lib/engines/quarto.js";
const chunks = Number(process.argv[2] || 200);
if (!Number.isInteger(chunks) || chunks < 1 || chunks > 10000) throw new Error("Expected 1–10000 chunks");
const source = Array.from({ length:chunks }, (_, i) => `## Section ${i}\n\nText around the analysis.\n\n\`\`\`{python}\n#| label: fig-${i}\nprint(${i})\n\`\`\`\n`).join("\n");
const samples = [];
for (let i = 0; i < 25; i++) {
  const start = performance.now();
  const parsed = parseQuarto(source);
  const draft = composeDraft(source);
  if (parsed.cells.length !== chunks || !draft) throw new Error("Incomplete draft");
  if (i >= 5) samples.push(performance.now() - start);
}
samples.sort((a,b) => a-b);
console.log(JSON.stringify({chunks, sourceBytes:Buffer.byteLength(source), iterations:samples.length, medianMs:samples[Math.floor(samples.length/2)], p95Ms:samples[Math.floor(samples.length*.95)]}, null, 2));
