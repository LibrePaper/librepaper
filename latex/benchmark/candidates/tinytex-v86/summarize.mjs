import { createHash } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';
import { CASES } from '../../corpus.mjs';
import { inspectPDF } from '../../results.mjs';
const root = fileURLToPath(new URL('.', import.meta.url));
const read = p => JSON.parse(readFileSync(root + p));
const report = read('results/report.json');
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const normalize = text => text.replace(/\s+/g, ' ').trim();
const summary = {
  date: report.date, inputAssets: read('assets-lock.json'),
  image: read('assets/image-receipt.json'), filesystem: read('assets/filesystem-receipt.json'),
  memoryConfiguredBytes: report.memoryConfiguredBytes,
  boot: { seconds: report.boot.seconds, bytes: report.boot.network.bytes },
  versions: report.versions, error: report.error || null,
  documentNetwork: report.documentNetwork, cases: [],
};
for (const c of report.cases) {
  const example = CASES.find(e => e.id === c.id);
  const stem = example.main.replace(/\.tex$/, '');
  const nativeDir = root + 'results/' + c.id + '/native/';
  const reference = c.native ? inspectPDF(nativeDir + stem + '.pdf') : null;
  const nativeBbl = existsSync(nativeDir + stem + '.bbl') ? readFileSync(nativeDir + stem + '.bbl') : null;
  const phases = c.phases.map(p => {
    const dir = root + 'results/' + c.id + '/' + p.phase + '/';
    const pdf = p.pdf ? inspectPDF(dir + stem + '.pdf') : null;
    const bbl = existsSync(dir + stem + '.bbl') ? readFileSync(dir + stem + '.bbl') : null;
    const sync = existsSync(dir + stem + '.synctex.gz') ? gunzipSync(readFileSync(dir + stem + '.synctex.gz')).toString('utf8') : '';
    return {
      phase: p.phase, status: p.status, seconds: p.seconds, bytes: p.network.bytes,
      cumulativeBytes: p.network.lifetime?.bytes,
      pdf: p.pdf, pages: p.pages, textAgreement: p.textAgreement,
      normalizedTextMatchesNative: p.phase !== 'edit' && pdf && reference ? normalize(pdf.text) === normalize(reference.text) : null,
      bibliographyMatchesNative: bbl && nativeBbl ? bbl.equals(nativeBbl) : null,
      bibliographySha256: bbl ? sha(bbl) : null,
      syncTexHeaderValid: sync.startsWith('SyncTeX Version:1'),
      warnings: p.warnings, biberInvoked: p.biberInvoked, editVisible: p.editVisible,
    };
  });
  summary.cases.push({ id: c.id, engine: c.engine, treeSha256: c.treeSha256, native: c.native, phases, engineOnly: c.engineOnly });
}
writeFileSync(root + 'summary.json', JSON.stringify(summary, null, 2) + '\n');
console.log(JSON.stringify({ boot: summary.boot, documentNetwork: summary.documentNetwork, cases: summary.cases }, null, 2));
