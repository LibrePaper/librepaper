import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
const root = fileURLToPath(new URL('.', import.meta.url));
const report = JSON.parse(readFileSync(join(root, 'results/report.json')));
const sha = file => createHash('sha256').update(readFileSync(file)).digest('hex');
const example = report.cases[0];
if (!example.verification?.ok || !report.invalidTexRejected || report.error) throw Error('The run has not passed validation');
const summary = {
  date: report.date, runtime: report.runtime.assets,
  boundaries: {
    browser: 'Chromium, fresh profile; one engine and emulator retained across the four phases',
    timing: 'Browser invocation through output collection; source tree staging included; native builds excluded',
    network: 'Local HTTP response body bytes, uncompressed; excludes hosted CheerpX runtime and HTTP headers',
    memory: 'Peak browser memory not measured',
  },
  phases: example.phases.map(phase => ({
    phase: phase.phase, seconds: phase.elapsedMs / 1000, pages: phase.pages,
    treeSha256: phase.treeSha256, nativeTools: phase.native.tools,
    texPassesSeconds: phase.timings.texPassesSeconds,
    texInitializationSeconds: phase.timings.initializationSeconds,
    biber: phase.biber, localResponseBytes: phase.network.bytes,
    cumulativeLocalResponseBytes: phase.network.lifetime.bytes,
    bibliographySha256: sha(join(root, 'results', example.id, phase.phase, '91-sorting-schemes.bbl')),
    checks: example.verification.phases.find(row => row.phase === phase.phase).checks,
  })),
  invalidTexRejected: report.invalidTexRejected,
};
writeFileSync(join(root, 'summary.json'), JSON.stringify(summary, null, 2) + '\n');
console.log(summary.phases.map(p => p.phase + ': ' + p.seconds.toFixed(2) + 's; ' + (p.biber?.count || 0) + ' Biber invocation(s)').join('\n'));
