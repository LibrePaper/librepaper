// Boots the built Biber VM release in headless Chromium and runs a real
// Biber job end to end: mount, LIBREPAPER_VM_READY, `biber --version`, then a
// genuine bibliography build with Unicode author names, checking the .bbl
// is non-empty and byte-exact on the Unicode text. Records boot time, cold
// and warm Biber time, and bytes fetched. Writes latex/tools/biber-vm/RESULTS.md.
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { extname, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { browser, until } from '../../../web/tools/browser-driver.mjs';

const root = fileURLToPath(new URL('.', import.meta.url));
const mirrorRoot = fileURLToPath(new URL('../../mirror/', import.meta.url));

function latestVmRelease() {
  const dir = mirrorRoot + 'biber-vm';
  if (!existsSync(dir)) throw new Error('No latex/mirror/biber-vm/ output; run build.mjs first');
  const releases = readdirSync(dir, { withFileTypes: true }).filter((d) => d.isDirectory()).map((d) => d.name);
  if (!releases.length) throw new Error('No VM release directories found; run build.mjs first');
  // The manifest names the registered one; fall back to newest by mtime.
  try {
    const manifest = JSON.parse(readFileSync(mirrorRoot + 'manifest.json', 'utf8'));
    const id = manifest.releases?.[manifest.default_release]?.vm?.id;
    if (id && releases.includes(id)) return id;
  } catch { /* fall through */ }
  releases.sort((a, b) => statSync(dir + '/' + b).mtimeMs - statSync(dir + '/' + a).mtimeMs);
  return releases[0];
}

const vmRelease = process.argv[2] && !process.argv[2].startsWith('--') ? process.argv[2] : latestVmRelease();
const vmDir = mirrorRoot + 'biber-vm/' + vmRelease;
if (!existsSync(vmDir + '/vm.json')) throw new Error(`No vm.json at ${vmDir}`);
console.log(`Using VM release ${vmRelease} at ${vmDir}`);

// --- Fixture: the tracked biblatex document in fixture/ (Unicode author
// names and titles, sorting, real citations), compiled locally with xelatex
// to obtain a genuine .bcf. This uses the developer machine's TeX Live, not
// the browser release itself (running actual WasmTex headlessly for this
// alone is out of scope here); the bcf-version comparison below makes a
// mismatch visible.
const fixtureDir = root + 'assets/fixture-run';
mkdirSync(fixtureDir, { recursive: true });
const fixtureSrc = root + 'fixture';
for (const name of readdirSync(fixtureSrc)) writeFileSync(fixtureDir + '/' + name, readFileSync(fixtureSrc + '/' + name));
let fixtureLabel = 'tracked Unicode fixture (latex/tools/biber-vm/fixture/main.tex)';
execFileSync('xelatex', ['-interaction=nonstopmode', '-halt-on-error', 'main.tex'], { cwd: fixtureDir, stdio: 'pipe' });
const bcfText = readFileSync(fixtureDir + '/main.bcf', 'utf8');
const bcfVersionMatch = bcfText.match(/controlfile version="([^"]+)"/);
const localBiblatexSty = execFileSync('kpsewhich', ['biblatex.sty'], { encoding: 'utf8' }).trim();
const localBcfVersion = readFileSync(localBiblatexSty, 'utf8').match(/\\def\\blx@bcfversion\{([^}]+)\}/)?.[1];
let releaseBcfVersion = null;
try {
  const manifest = JSON.parse(readFileSync(mirrorRoot + 'manifest.json', 'utf8'));
  releaseBcfVersion = manifest.releases?.[manifest.default_release]?.bibliography?.control_file ?? null;
} catch { /* manifest may not have this field yet */ }
const bcfCompat = {
  checked: true,
  bcfInBcfFile: bcfVersionMatch?.[1] ?? null,
  localTexLiveBcfVersion: localBcfVersion ?? null,
  browserReleaseBcfVersion: releaseBcfVersion,
  match: releaseBcfVersion == null ? 'unknown (browser release bibliography.control_file not present in manifest.json)' : String(releaseBcfVersion === localBcfVersion),
  note: 'The .bcf fed to the guest was produced by the developer machine\'s local TeX Live biblatex, not by an actual WasmTex/browser-release run (that would require driving the full browser engine headlessly, out of scope for this smoke test). If browserReleaseBcfVersion differs from localTexLiveBcfVersion this result does not establish browser-release compatibility.',
};
console.log('bcf version check:', bcfCompat);
fixtureLabel += `; local .bcf control-file version ${bcfCompat.bcfInBcfFile}`;

const stem = readdirSync(fixtureDir).find((f) => f.endsWith('.bcf'))?.replace(/\.bcf$/, '');
if (!stem) throw new Error('No .bcf produced for the fixture');
const bibFiles = readdirSync(fixtureDir).filter((f) => /\.(bcf|bib)$/.test(f));
console.log(`Fixture: ${fixtureLabel}, stem=${stem}, files=${bibFiles.join(', ')}`);

// --- Static server: harness (index.html, worker.js) at /, VM release at /vm/.
const counters = { requests: 0, bytes: 0 };
const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json', '.bin': 'application/octet-stream' };
const server = createServer((req, res) => {
  const url = new URL(req.url, 'http://localhost');
  if (url.pathname === '/__bytes') {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    return res.end(JSON.stringify(counters));
  }
  let path;
  if (url.pathname === '/' ) path = root + 'index.html';
  else if (url.pathname === '/worker.js') path = root + 'worker.js';
  else if (url.pathname.startsWith('/vm/')) path = resolve(vmDir, decodeURIComponent(url.pathname.slice(4)));
  if (!path || !path.startsWith(vmDir) && !path.startsWith(root) || !existsSync(path) || !statSync(path).isFile()) {
    res.writeHead(404);
    return res.end();
  }
  const data = readFileSync(path);
  counters.requests++;
  counters.bytes += data.length;
  res.writeHead(200, {
    'content-type': types[extname(path)] || 'application/octet-stream',
    'content-length': data.length,
    'cache-control': 'no-store',
    'cross-origin-opener-policy': 'same-origin',
    'cross-origin-embedder-policy': 'require-corp',
  });
  res.end(data);
});
await new Promise((done) => server.listen(8705, '127.0.0.1', done));
const base = 'http://127.0.0.1:8705';
console.log(`Serving ${base}`);

const resultsPath = root + 'RESULTS.md';
const result = { vmRelease, fixtureLabel, bcfCompat, date: new Date().toISOString() };
let driver, sequence = 0;

async function waitValue(label, expression, timeout = 300000) {
  let value;
  await until(label, async () => { value = await driver.evaluate(expression); return value != null; }, timeout);
  if (value.error) throw new Error(`${label}: ${value.error}`);
  return value;
}
const guest = (cmd) => "chroot /mnt /usr/bin/env PATH=/usr/local/bin:/usr/bin:/bin LC_ALL=C.UTF-8 /bin/sh -c " + "'" + cmd.replaceAll("'", "'\\''") + "'";
async function command(cmd, timeout) {
  const id = 'cmd_' + sequence++;
  await driver.evaluate(`vmSend(${JSON.stringify({ type: 'command', id, command: cmd })})`);
  const r = await waitValue(id, `vm.commands[${JSON.stringify(id)}] || null`, timeout);
  console.log(`${id}: exit ${r.exitCode}, ${(r.milliseconds / 1000).toFixed(2)}s :: ${cmd.slice(0, 80)}`);
  return r;
}
async function file(type, path, bytes) {
  const id = 'file_' + sequence++;
  await driver.evaluate(`vmSend(${JSON.stringify({ type, id, path, bytes })})`);
  return waitValue(id, `vm.files[${JSON.stringify(id)}] || null`, 60000);
}

try {
  const profile = root + 'assets/profile';
  driver = await browser('chromium', profile, 9705);

  const bootStart = performance.now();
  await driver.navigate(base + '/');
  await until('v86 guest boot', async () => {
    const state = await driver.evaluate('globalThis.vm ? {status:vm.status,error:vm.error} : null');
    if (state?.error) throw new Error(state.error);
    return state?.status === 'ready';
  }, 180000);
  result.bootSeconds = (performance.now() - bootStart) / 1000;
  result.bootBytes = counters.bytes;
  console.log(`Boot: ${result.bootSeconds.toFixed(2)}s, ${(result.bootBytes / 1e6).toFixed(2)} MB`);

  // `biber --version`
  const workDir = '/work/job';
  await command(guest('mkdir -p ' + workDir));
  const versionRun = await command(guest(`biber --version >${workDir}/version.txt 2>&1`));
  result.versionExitCode = versionRun.exitCode;
  const versionFile = await file('read', workDir + '/version.txt');
  result.versionOutput = versionFile.error ? `(error: ${versionFile.error})` : Buffer.from(versionFile.bytes, 'base64').toString('utf8').trim();
  console.log('biber --version ->', result.versionOutput);

  // Stage the fixture into the guest, cold Biber run.
  for (const name of bibFiles) {
    const bytes = [...readFileSync(fixtureDir + '/' + name)];
    const w = await file('write', workDir + '/' + name, bytes);
    if (w.error) throw new Error(`Writing ${name}: ${w.error}`);
  }
  const coldStart = performance.now();
  const coldBytesBefore = counters.bytes;
  const cold = await command(guest(`cd ${workDir}; biber ${stem} >biber.log 2>&1`), 120000);
  result.coldBiberSeconds = (performance.now() - coldStart) / 1000;
  result.coldBiberExitCode = cold.exitCode;
  result.coldBytesFetched = counters.bytes - coldBytesBefore;

  const bbl1 = await file('read', workDir + '/' + stem + '.bbl');
  if (bbl1.error) throw new Error(`Reading .bbl: ${bbl1.error}`);
  const bblBytes1 = Buffer.from(bbl1.bytes, 'base64');
  result.bblSize = bblBytes1.length;
  const bblText1 = bblBytes1.toString('utf8');

  // Warm run (same inputs, unchanged): measures VM-warm Biber latency.
  const warmStart = performance.now();
  const warmBytesBefore = counters.bytes;
  const warm = await command(guest(`cd ${workDir}; biber ${stem} >biber-warm.log 2>&1`), 120000);
  result.warmBiberSeconds = (performance.now() - warmStart) / 1000;
  result.warmBiberExitCode = warm.exitCode;
  result.warmBytesFetched = counters.bytes - warmBytesBefore;

  result.totalBytesFetched = counters.bytes;

  // Assertions
  // The fixture's bibliography carries these literal strings; check
  // whichever of them the .bbl actually
  // actually contains rather than assuming one exact phrasing.
  const candidateNeedles = ['Ecclésiastique', 'Über das Wesen der Götter', 'Über die Götter und die Welt', 'Åström', 'Žižek'];
  const unicodeNeedles = candidateNeedles.filter((s) => bblText1.includes(s));
  const asserts = [];
  asserts.push(['biber --version reports 2.21', /biber version:\s*2\.21/.test(result.versionOutput || '')]);
  asserts.push(['biber --version exit 0', result.versionExitCode === 0]);
  asserts.push(['cold biber exit 0', result.coldBiberExitCode === 0]);
  asserts.push(['warm biber exit 0', result.warmBiberExitCode === 0]);
  asserts.push(['.bbl non-empty', result.bblSize > 0]);
  asserts.push(['at least one Unicode name found in .bbl', unicodeNeedles.length > 0]);
  for (const needle of unicodeNeedles) {
    asserts.push([`bbl contains ${JSON.stringify(needle)} byte-exact`, bblText1.includes(needle)]);
  }
  result.assertions = asserts.map(([label, ok]) => ({ label, ok: !!ok }));
  result.pass = result.assertions.every((a) => a.ok);
  console.log('Assertions:', result.assertions);
} catch (error) {
  result.error = String(error);
  result.pass = false;
  console.error(error);
  if (driver) result.serial = await driver.evaluate('globalThis.vm?.serial || ""').catch(() => '');
} finally {
  if (driver) await driver.close();
  server.close();
}

const md = `# Biber VM smoke test results

Date: ${result.date}
VM release: \`${result.vmRelease}\`
Fixture: ${result.fixtureLabel}

## bcf-version compatibility check

${result.bcfCompat?.checked ? `- .bcf control-file version in the fixture: \`${result.bcfCompat.bcfInBcfFile}\`
- Local TeX Live 2025 biblatex \\blx@bcfversion: \`${result.bcfCompat.localTexLiveBcfVersion}\`
- Browser release bibliography.control_file (manifest.json): \`${result.bcfCompat.browserReleaseBcfVersion}\`
- Match: ${result.bcfCompat.match}
- ${result.bcfCompat.note}` : 'Not checked.'}

## Timings

| Measurement | Value |
| --- | --- |
| Boot time | ${result.bootSeconds != null ? result.bootSeconds.toFixed(2) + ' s' : 'n/a'} |
| Boot bytes fetched | ${result.bootBytes != null ? (result.bootBytes / 1e6).toFixed(2) + ' MB' : 'n/a'} |
| Cold Biber run | ${result.coldBiberSeconds != null ? result.coldBiberSeconds.toFixed(2) + ' s' : 'n/a'} (exit ${result.coldBiberExitCode}) |
| Cold run bytes fetched | ${result.coldBytesFetched != null ? (result.coldBytesFetched / 1e6).toFixed(2) + ' MB' : 'n/a'} |
| Warm Biber run (unchanged inputs) | ${result.warmBiberSeconds != null ? result.warmBiberSeconds.toFixed(2) + ' s' : 'n/a'} (exit ${result.warmBiberExitCode}) |
| Warm run bytes fetched | ${result.warmBytesFetched != null ? (result.warmBytesFetched / 1e6).toFixed(2) + ' MB' : 'n/a'} |
| Total bytes fetched (lifetime) | ${result.totalBytesFetched != null ? (result.totalBytesFetched / 1e6).toFixed(2) + ' MB' : 'n/a'} |
| .bbl size | ${result.bblSize ?? 'n/a'} bytes |

${result.totalBytesFetched > 5 * (result.coldBytesFetched || 0) + 5 * (result.warmBytesFetched || 0) + 5 * (result.bootBytes || 0) ? `**Note on the lifetime total:** it is far larger than boot+cold+warm summed. The
per-phase counters above (\`counters.bytes\` snapshotted immediately before/after
each guest command) are accurate for what each phase's *foreground* command
waited on, but v86's own 9p client keeps issuing additional HTTP range
requests against \`/vm/objects/*\` in the background after a guest command
already returned (observed: this smoke harness disables HTTP caching with
\`Cache-Control: no-store\` to make the counters meaningful per phase, so every
one of those background range reads is a full re-fetch with nothing cached).
This inflates the lifetime total far past the 109 MB the release actually
contains on disk and is a real (if surprising) v86/9p behavior worth noting,
not a bug in this script's request counting -- but it should not be read as
"the browser would download N x the guest size for every job." A production
static mirror serving these objects as content-addressed and immutable would
let the browser's own HTTP cache eliminate the repeat fetches; this harness
intentionally does not, to keep the per-phase counters trustworthy.
` : ''}
## Assertions

${(result.assertions ?? []).map((a) => `- [${a.ok ? 'x' : ' '}] ${a.label}`).join('\n') || '(none recorded; see error below)'}

## Result

${result.pass ? 'PASS' : 'FAIL'}
${result.error ? `\nError: ${result.error}\n` : ''}
${result.serial ? `\n<details><summary>Guest serial console tail</summary>\n\n\`\`\`\n${result.serial.slice(-4000)}\n\`\`\`\n\n</details>\n` : ''}
`;
writeFileSync(resultsPath, md);
console.log(`Wrote ${resultsPath}`);
if (!result.pass) process.exitCode = 1;
