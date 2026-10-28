import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync, spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

// === LaTeX release checker ===

const directory = mkdtempSync(join(tmpdir(), 'librepaper-mirror-test-'));
const checker = fileURLToPath(new URL('./mirror.mjs', import.meta.url));
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

// A mirror is `<id>/` release directories; id is the SHA-256 of MANIFEST.json
// and every path in release.json is relative to that directory.
let mirrorTest;
let release;
let id;
let remoteServer;

function assetTest(url, content) {
  const bytes = Buffer.from(content);
  mkdirSync(join(release, url, '..'), { recursive: true });
  writeFileSync(join(release, url), bytes);
  return { url, size: bytes.length, sha256: digest(bytes) };
}

// Two tiny bundle "tars" (any bytes; check-mirror
// only hashes them) under a release's own `bundles/` directory, named by
// digest as the real ones would be.
function bundlesFixture() {
  const put = (content, slug) => {
    const bytes = Buffer.from(content);
    return assetTest(`bundles/b/${digest(bytes)}/${slug}.tar`, bytes);
  };
  const coreFile = put('fixture core bundle', 'core');
  const tikzFile = put('fixture tikz bundle', 'tex-latex-tikz');
  const index = {
    schemaVersion: 1,
    snapshot: 'texlive-20260301',
    sourceDateEpoch: 1,
    bundles: {
      core: { url: coreFile.url.slice('bundles/'.length), size: coreFile.size, sha256: coreFile.sha256 },
      'tex/latex/tikz': { url: tikzFile.url.slice('bundles/'.length), size: tikzFile.size, sha256: tikzFile.sha256 },
    },
    files: { 'tex/latex/base/article.cls': 'core', 'tex/generic/pgf/basiclayer/pgfcore.code.tex': 'tex/latex/tikz' },
  };
  const indexFile = assetTest('bundles/bundles.json', JSON.stringify(index));
  return { index: indexFile.url, sha256: indexFile.sha256, snapshot: index.snapshot, count: 2, bytes: coreFile.size + tikzFile.size };
}

/// A fresh, complete mirror with one release; `mutate(entry)` may break the
/// release.json entry before it is written, and returns nothing.
function buildCheckTest(mutate = () => {}) {
  mirrorTest = mkdtempSync(join(directory, 'mirror-'));
  const manifest = Buffer.from(`{"label":"${Math.random()}"}\n`);
  id = digest(manifest);
  release = join(mirrorTest, id);
  mkdirSync(release, { recursive: true });
  writeFileSync(join(release, 'MANIFEST.json'), manifest);
  const entry = {
    format: 2,
    id,
    engines: { pdftex: { worker: 'worker.js', files: ['worker.js'] } },
    files: { 'worker.js': assetTest('worker.js', 'worker') },
    bundles: bundlesFixture(),
  };
  mutate(entry);
  writeFileSync(join(release, 'release.json'), JSON.stringify(entry));
}

function checkRun(expected, message, target = mirrorTest) {
  const result = spawnSync(process.execPath, [checker, 'check', target], { encoding: 'utf8' });
  if (result.error) throw result.error;
  assert.equal(result.status, expected, result.stderr || result.stdout);
  if (message) assert.match(result.stderr, message);
}

function checkRunAsync(expected, message, target) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [checker, 'check', target], { encoding: 'utf8' });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8').on('data', (chunk) => { stdout += chunk; });
    child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
    child.once('error', reject);
    child.once('close', (status) => {
      try {
        assert.equal(status, expected, stderr || stdout);
        if (message) assert.match(stderr, message);
        resolve();
      } catch (error) {
        reject(error);
      }
    });
  });
}

try {
  buildCheckTest();
  checkRun(0);
  checkRun(0, undefined, release); // a single release directory
  let corruptWorker = false;
  remoteServer = createServer((request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    if (path === `/${id}/release.json`) return response.end(readFileSync(join(release, 'release.json')));
    if (path === `/${id}/worker.js`) return response.end(corruptWorker ? 'corrupt' : readFileSync(join(release, 'worker.js')));
    response.writeHead(404).end();
  });
  remoteServer.listen(0, '127.0.0.1');
  await new Promise((resolve, reject) => {
    remoteServer.once('listening', resolve);
    remoteServer.once('error', reject);
  });
  const remote = `http://127.0.0.1:${remoteServer.address().port}/${id}/`;
  await checkRunAsync(0, undefined, remote);
  corruptWorker = true;
  await checkRunAsync(1, /worker payload does not match/, remote);
  await new Promise((resolve, reject) => remoteServer.close((error) => error ? reject(error) : resolve()));
  remoteServer = null;
  buildCheckTest((entry) => { entry.format = 1; });
  checkRun(1, /unsupported release format/);
  buildCheckTest((entry) => { delete entry.engines.pdftex; });
  checkRun(1, /pdfTeX worker specification/);
  buildCheckTest((entry) => { entry.engines.pdftex.files = []; });
  checkRun(1, /pdfTeX engine has no valid worker inventory/);
  buildCheckTest((entry) => { entry.engines.xetex = { worker: 'xetex.worker.js', files: [] }; });
  checkRun(1, /engine xetex has no valid worker inventory/);
  buildCheckTest((entry) => { entry.files['worker.js'].size = 0; });
  checkRun(1, /non-empty file record/);
  buildCheckTest((entry) => { entry.files['worker.js'].url = '..%2foutside.js'; });
  checkRun(1, /unsafe URL/);
  buildCheckTest((entry) => { entry.bundles = null; });
  checkRun(1, /has no bundles/);
  buildCheckTest();
  rmSync(join(release, 'worker.js'));
  checkRun(1, /worker\.js references a file that will not be published/);
  buildCheckTest();
  writeFileSync(join(release, 'worker.js'), 'broken');
  checkRun(1, /integrity mismatch/);
  buildCheckTest();
  const outside = join(directory, 'outside-worker.js');
  writeFileSync(outside, 'worker');
  rmSync(join(release, 'worker.js'));
  symlinkSync(outside, join(release, 'worker.js'));
  checkRun(1, /symbolic link/);

  // The directory name is the SHA-256 of MANIFEST.json.
  buildCheckTest();
  writeFileSync(join(release, 'MANIFEST.json'), 'changed');
  checkRun(1, /directory does not match the SHA-256 of MANIFEST\.json/);
  buildCheckTest();
  rmSync(join(release, 'MANIFEST.json'));
  checkRun(1, /is missing MANIFEST\.json/);
  mirrorTest = mkdtempSync(join(directory, 'empty-'));
  checkRun(1, /no release directories/);

  // bundles.json's own digest must match what
  // the release pins, and every bundle path it names must actually be on
  // disk -- both independent of `release.files`, which only proves the
  // files the importer copied are intact, not that bundles.json still
  // agrees with them.
  buildCheckTest();
  writeFileSync(join(release, 'bundles', 'bundles.json'), 'tampered');
  checkRun(1, /bundles index SHA-256 mismatch/);
  buildCheckTest();
  const [coreName, coreBundle] = Object.entries(JSON.parse(readFileSync(join(release, 'bundles', 'bundles.json'), 'utf8')).bundles)[0];
  writeFileSync(join(release, 'bundles', coreBundle.url), 'corrupted bundle bytes');
  checkRun(1, new RegExp(`bundle ${coreName} integrity mismatch`));
  buildCheckTest();
  rmSync(join(release, 'bundles', coreBundle.url));
  checkRun(1, new RegExp(`bundle ${coreName} references a file that will not be published`));

  buildCheckTest();
  const indexPath = join(release, 'bundles', 'bundles.json');
  const releasePath = join(release, 'release.json');
  const brokenIndex = JSON.parse(readFileSync(indexPath, 'utf8'));
  brokenIndex.files['tex/latex/base/article.cls'] = 'missing-bundle';
  const brokenIndexBytes = JSON.stringify(brokenIndex);
  writeFileSync(indexPath, brokenIndexBytes);
  const brokenRelease = JSON.parse(readFileSync(releasePath, 'utf8'));
  brokenRelease.bundles.sha256 = digest(brokenIndexBytes);
  writeFileSync(releasePath, JSON.stringify(brokenRelease));
  checkRun(1, /names unknown bundle "missing-bundle"/);

  console.log('mirror preflight: release shape, worker payloads, path containment, missing assets, corruption, and bundle mismatches checked');
} finally {
  if (remoteServer) await new Promise((resolve) => remoteServer.close(resolve));
  rmSync(directory, { recursive: true, force: true });
}
