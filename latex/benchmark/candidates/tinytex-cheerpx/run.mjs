import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';
import { browser, until } from '../../../../web/tools/browser-driver.mjs';
import { CASES, treeOf, treeDigest } from '../../corpus.mjs';
import { inspectPDF, textAgreement, warnings } from '../../results.mjs';
import { server } from './server.mjs';
const root = fileURLToPath(new URL('.', import.meta.url));
const results = root + 'results';
mkdirSync(results, { recursive: true });
const base = 'http://127.0.0.1:8705';
const q = s => "'" + s.replaceAll("'", "'\\''") + "'";
let driver, sequence = 0;
const report = { date: new Date().toISOString(), runtime: 'CheerpX 1.2.8', cases: [] };
const counters = async () => (await fetch(base + '/__bytes')).json();
const reset = () => fetch(base + '/__reset');
async function waitValue(label, expression, timeout = 600000) {
  let value;
  await until(label, async () => {
    value = await driver.evaluate(expression);
    return value != null;
  }, timeout);
  if (value.error) throw new Error(`${label}: ${value.error}`);
  return value;
}
async function command(command, timeout) {
  const id = 'cmd_' + sequence++;
  await driver.evaluate(`vmSend(${JSON.stringify({ type: 'command', id, command })})`);
  const result = await waitValue(id, `vm.commands[${JSON.stringify(id)}] || null`, timeout);
  console.log(`${id}: exit ${result.exitCode}, ${(result.milliseconds / 1000).toFixed(2)}s`);
  return result;
}
const guest = cmd => cmd;
async function file(type, path, bytes) {
  const id = 'file_' + sequence++;
  await driver.evaluate(`vmSend(${JSON.stringify({ type, id, path, bytes })})`);
  return waitValue(id, `vm.files[${JSON.stringify(id)}] || null`, 120000);
}
async function boot() {
  await reset();
  const start = performance.now();
  await driver.navigate(base + '/');
  await until('CheerpX initialization', async () => {
    const state = await driver.evaluate('globalThis.vm ? {status:vm.status,error:vm.error} : null');
    if (state?.error) return true;
    return state?.status === 'ready';
  }, 180000);
  const bootError = await driver.evaluate('vm.error');
  if (bootError) throw new Error(bootError);
  const measurement = { seconds: (performance.now() - start) / 1000, network: await counters() };
  console.log(`boot: ${measurement.seconds.toFixed(2)}s, ${(measurement.network.bytes / 1e6).toFixed(2)} MB`);
  return measurement;
}
function compileCommand(example) {
  const flag = { pdflatex: '-pdf', xelatex: '-xelatex', lualatex: '-lualatex' }[example.engine];
  // CheerpX retains stale mtimes on overwritten logs, even on ext2. Recreate
  // only the diagnostic log for each pass; keep all incremental auxiliary data.
  const engineCommand = 'rm -f '+q(example.main.replace(/\.tex$/, '.log'))+' && '+example.engine+' -no-shell-escape %O %S';
  return `latexmk -norc ${flag} -interaction=nonstopmode -halt-on-error -file-line-error -synctex=1 ${q('-' + example.engine + '=' + engineCommand)} ${q(example.main)} >pipeline.log 2>&1`;
}
function inputFiles(tree) { return { ...Object.fromEntries(Object.entries(tree.texts).map(([k,v]) => [k, Buffer.from(v)])), ...Object.fromEntries(Object.entries(tree.assets).map(([k,v]) => [k, Buffer.from(v)])) }; }
async function stage(example, tree) {
  const dir = '/work/' + example.id;
  const folders = new Set([dir, ...Object.keys(inputFiles(tree)).map(p => dirname(dir + '/' + p))]);
  const made = await command(guest('mkdir -p ' + [...folders].map(q).join(' ')));
  if (made.exitCode) throw new Error('Cannot create project directory');
  for (const [path, bytes] of Object.entries(inputFiles(tree))) await file('write', dir + '/' + path, [...bytes]);
}
async function compile(example, phase, reference) {
  await reset();
  const start = performance.now();
  const run = await command(guest(`cd ${q('/work/' + example.id)}; ${compileCommand(example)}`));
  const seconds = (performance.now() - start) / 1000;
  const network = await counters();
  const dir = join(results, example.id, phase);
  mkdirSync(dir, { recursive: true });
  const stem = example.main.replace(/\.tex$/, '');
  const outputs = {};
  for (const name of ['pipeline.log', stem + '.log', stem + '.pdf', stem + '.bbl', stem + '.blg', stem + '.synctex.gz']) {
    try {
      const value = await file('read', '/work/' + example.id + '/' + name);
      const bytes = Buffer.from(value.bytes, 'base64');
      writeFileSync(join(dir, name), bytes);
      outputs[name] = bytes.length;
    } catch (error) { outputs[name] = { error: String(error) }; }
  }
  const pdf = typeof outputs[stem + '.pdf'] === 'number' ? inspectPDF(join(dir, stem + '.pdf')) : { pdf: false, pages: 0, text: '' };
  const log = typeof outputs['pipeline.log'] === 'number' ? readFileSync(join(dir, 'pipeline.log'), 'utf8') : '';
  // Earlier passes normally contain unresolved citations. Review the final TeX
  // pass, while retaining the full pipeline log for actual backend failures.
  const finalLog = typeof outputs[stem + '.log'] === 'number' ? readFileSync(join(dir, stem + '.log'), 'utf8') : log;
  const reviewWarnings = warnings(finalLog);
  const agreement = reference && phase === 'first' && pdf.pdf ? textAgreement(reference.text, pdf.text) : null;
  const editVisible = phase === 'edit' && pdf.pdf ? /LibrePaper emulator edit\./.test(pdf.text) : null;
  const measurement = { phase, seconds, guestCommandSeconds: run.milliseconds / 1000, exitCode: run.exitCode, network, outputs, pdf: pdf.pdf, pages: pdf.pages, warnings: reviewWarnings, textAgreement: agreement, referencePages: reference?.pages, editVisible,
    biberInvoked: /Run number \d+ of rule ['"]biber /.test(log),
    status: run.exitCode || !pdf.pdf ? 'failed' : reviewWarnings.length || editVisible === false || (agreement != null && (agreement < 0.99 || pdf.pages !== reference.pages)) ? 'needs-review' : 'pdf-produced' };
  console.log(`${example.id}/${phase}: ${measurement.status}, ${seconds.toFixed(2)}s, ${(network.bytes / 1e6).toFixed(2)} MB, ${pdf.pages} pages`);
  return measurement;
}
function native(example, tree) {
 const dir=root+'assets/reference/'+example.id;
 return inspectPDF(join(dir, example.main.replace(/\.tex$/, '.pdf')));
}

try {
  const selected = process.argv.slice(2).filter(x => !x.startsWith('--'));
  const cases = (selected.length ? selected : ['multifile', 'biber-sorting']).map(id => {
    const found = CASES.find(c => c.id === id); if (!found) throw new Error(`Unknown case ${id}`); return found;
  });
  const profile = root + 'profiles/chromium';
  rmSync(profile, { recursive: true, force: true });
  driver = await browser('chromium', profile, 9705);
  report.boot = await boot();
  if (!process.argv.includes('--boot-only')) for (const example of cases) {
    const tree = treeOf(example);
    const reference = native(example, tree);
    await stage(example, tree);
    const record = { id: example.id, engine: example.engine, treeSha256: treeDigest(tree), native: reference ? { pdf: reference.pdf, pages: reference.pages } : null, phases: [] };
    report.cases.push(record);
    record.phases.push(await compile(example, 'first', reference));
    const edited = tree.texts[tree.main].replace(/\\end\{document\}(?![\s\S]*\\end\{document\})/, '\n\\par LibrePaper emulator edit.\n\\end{document}');
    await file('write', '/work/' + example.id + '/' + tree.main, [...Buffer.from(edited)]);
    record.phases.push(await compile(example, 'edit', reference));
    // Separate TeX execution from latexmk's Perl startup/orchestration cost.
    if (record.phases.at(-1).exitCode === 0) {
      const onePass = await command(guest(`cd ${q('/work/' + example.id)}; ${example.engine} -no-shell-escape -interaction=nonstopmode -halt-on-error -synctex=1 ${q(example.main)} >engine-only.log 2>&1`));
      record.engineOnly = { seconds: onePass.milliseconds / 1000, exitCode: onePass.exitCode, description: 'One already-settled TeX pass, no edit or bibliography invocation' };
    }
    if(example.id==='biber-sorting' && record.phases.at(-1).exitCode===0) {
      const name='biblatex-examples.bib';
      const updated=tree.texts[name].replace(/(@book\{companion,[\s\S]*?\btitle\s*=\s*\{)/,'$1LibrePaper bibliography update: ');
      if(updated===tree.texts[name]) throw new Error('Bibliography edit did not change fixture');
      await file('write','/work/'+example.id+'/'+name,[...Buffer.from(updated)]);
      const changed=await compile(example,'bibliography-edit',reference);
      const pdf=inspectPDF(join(results,example.id,'bibliography-edit',example.main.replace(/\.tex$/,'.pdf')));
      // pdftotext -layout may wrap the edited title at a column boundary (the
      // observed output is `LibrePaper\nbibliography update:`). Treat runs of
      // whitespace as equivalent so this check tests the rendered title,
      // rather than the extractor's line-breaking choice.
      changed.bibliographyEditVisible=/LibrePaper\s+bibliography\s+update/.test(pdf.text);
      if(!changed.bibliographyEditVisible || !changed.biberInvoked) changed.status='needs-review';
      record.phases.push(changed);
    }
    writeFileSync(results + '/report.json', JSON.stringify(report, null, 2) + '\n');
  }
  // Probe versions after documents, so Biber/XeTeX do not prewarm the first
  // pdfLaTeX case or inflate its startup download with unused tools.
  report.documentNetwork = (await counters()).lifetime;
  const versions = await command(guest('(uname -m; pdflatex --version; xelatex --version; biber --version) >/work/versions.txt 2>&1'));
  report.versionsExitCode = versions.exitCode;
  report.versionProbeSeconds = versions.milliseconds / 1000;
  report.versions = Buffer.from((await file('read', '/work/versions.txt')).bytes, 'base64').toString('utf8');
  report.runtimeResources = await driver.evaluate('performance.getEntriesByType("resource").filter(r=>r.name.includes("leaningtech")).map(r=>({name:r.name,duration:r.duration,transferSize:r.transferSize,encodedBodySize:r.encodedBodySize}))');
  report.totalNetwork = (await counters()).lifetime;
  writeFileSync(results + '/report.json', JSON.stringify(report, null, 2) + '\n');
} catch (error) {
  report.error = String(error);
  console.error(error);
  if (driver) report.serial = await driver.evaluate('globalThis.vm?.serial || ""').catch(() => '');
  writeFileSync(results + '/report.json', JSON.stringify(report, null, 2) + '\n');
  process.exitCode = 1;
} finally {
  if (driver) { writeFileSync(results + '/serial.log', await driver.evaluate('globalThis.vm?.serial || ""').catch(() => '')); await driver.close(); }
  server.close();
}
