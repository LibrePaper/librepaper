import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { compileCorpus } from "./texlive.mjs";

const fixtureNames = ["article", "paper", "broken", "xetex", "unicode-fonts", "packages"];
const fakeEngine = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const name = path.basename(process.cwd());
if (process.env.FAKE_FAIL === name) process.exit(19);
if (process.env.FAKE_NO_OUTPUT === name) process.exit(0);
if (name === 'broken') {
  fs.writeFileSync('main.log', 'Undefined control sequence.\\nFile chapters/missing.tex not found.\\nFatal error occurred\\n');
  process.exit(1);
}
const pages = { article: 3, paper: 4, xetex: 1, 'unicode-fonts': 1, packages: 2 }[name];
const missing = name === 'unicode-fonts' && process.env.FAKE_MISSING_GLYPH ? 'Missing character: There is no Greek glyph.\\n' : '';
const output = process.env.FAKE_ZERO_PAGES === name ? '' : 'Output written on main.pdf (' + pages + ' pages, 1 byte).\\n';
fs.writeFileSync('main.log', missing + output);
fs.writeFileSync('main.pdf', 'pdf');
fs.writeFileSync('main.synctex.gz', 'synctex');
`;

async function fixture({ omitXeTeX = false, staleOutputs = false } = {}) {
  const root = await mkdtemp(join(tmpdir(), "librepaper-texlive-test-"));
  const corpus = join(root, "corpus");
  const bin = join(root, "bin");
  await mkdir(bin);
  for (const name of fixtureNames) {
    await mkdir(join(corpus, name), { recursive: true });
    await writeFile(join(corpus, name, "main.tex"), `fixture ${name}\n`);
    await mkdir(join(corpus, name, "logs"));
    await writeFile(join(corpus, name, "logs", "texlive.log"), `old ${name} log\n`);
  }
  if (staleOutputs) {
    await writeFile(join(corpus, "paper", "main.log"), "Output written on main.pdf (4 pages, stale).\\n");
    await writeFile(join(corpus, "paper", "main.pdf"), "stale PDF");
    await writeFile(join(corpus, "paper", "main.synctex.gz"), "stale SyncTeX");
  }
  await writeFile(join(corpus, "pages.json"), "old pages\n");
  for (const command of ["pdflatex", ...(!omitXeTeX ? ["xelatex"] : [])]) {
    const path = join(bin, command);
    await writeFile(path, fakeEngine);
    await chmod(path, 0o755);
  }
  const bibtex = join(bin, "bibtex");
  await writeFile(bibtex, `#!${process.execPath}\nprocess.exit(0);\n`);
  await chmod(bibtex, 0o755);
  return { root, corpus, env: { PATH: bin } };
}

test("TeX Live records only positive examples and the intentional broken fixture", async () => {
  const temp = await fixture();
  try {
    const record = compileCorpus({ corpus: temp.corpus, keepPdf: true, env: temp.env });
    assert.deepEqual(Object.keys(record), fixtureNames);
    assert.equal(record.article.pages, 3);
    assert.equal(record.broken.pages, 0);
    assert.equal(record["unicode-fonts"].engine, "xelatex");
    assert.equal(JSON.parse(await readFile(join(temp.corpus, "pages.json"), "utf8"))["unicode-fonts"].pages, 1);
    assert.equal(await readFile(join(temp.corpus, "unicode-fonts", "main.pdf"), "utf8"), "pdf");
    assert.match(await readFile(join(temp.corpus, "broken", "logs", "texlive.log"), "latin1"), /Fatal error occurred/);
  } finally {
    await rm(temp.root, { recursive: true, force: true });
  }
});

test("a missing engine, failed positive compile, or missing Unicode glyph preserves prior fixtures", async () => {
  for (const [options, env, expected] of [
    [{ omitXeTeX: true }, {}, /required TeX Live command not found: xelatex/],
    [{}, { FAKE_FAIL: "paper" }, /paper: pdflatex exited with status 19/],
    [{ staleOutputs: true }, { FAKE_NO_OUTPUT: "paper" }, /paper: pdflatex produced no main.log/],
    [{}, { FAKE_ZERO_PAGES: "paper" }, /paper: pdflatex log has no positive page count/],
    [{}, { FAKE_MISSING_GLYPH: "1" }, /XeTeX reported a missing glyph/],
  ]) {
    const temp = await fixture(options);
    try {
      assert.throws(() => compileCorpus({ corpus: temp.corpus, env: { ...temp.env, ...env } }), expected);
      assert.equal(await readFile(join(temp.corpus, "pages.json"), "utf8"), "old pages\n");
      for (const name of fixtureNames) {
        assert.equal(await readFile(join(temp.corpus, name, "logs", "texlive.log"), "utf8"), `old ${name} log\n`);
      }
    } finally {
      await rm(temp.root, { recursive: true, force: true });
    }
  }
});
