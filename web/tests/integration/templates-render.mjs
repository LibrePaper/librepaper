import assert from 'node:assert/strict';
import { readdir, readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFile, spawnSync } from 'node:child_process';
import { promisify } from 'node:util';
import { readFileSync } from 'node:fs';
import { fillTemplate } from '../../src/lib/starter.js';
import { call, handOver } from '../../src/lib/renderer-wasm.js';

const run = promisify(execFile);
const testTitle = 'A & B "C" \\ $x_1$ #1 *bold* 50%';
const testAuthor = "Ada O'Brien";

// Walk template directory recursively into [{ path, text }].
async function readTemplateFiles(basePath) {
  const files = [];
  async function walk(dir) {
    const entries = await readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = join(dir, entry.name);
      if (entry.isDirectory()) {
        await walk(fullPath);
      } else {
        const text = await readFile(fullPath, 'utf8');
        const relPath = fullPath.slice(basePath.length + 1);
        files.push({ path: relPath, text });
      }
    }
  }
  await walk(basePath);
  return files;
}

// Render markdown via WASM engine (citations.wasm).
async function renderMarkdown(mainFile, texts) {
  const binary = readFileSync(new URL('../../wasm/citations.wasm', import.meta.url));
  const engine = new WebAssembly.Instance(new WebAssembly.Module(binary), {}).exports;
  handOver(engine, { main: mainFile, texts, assets: {}, urls: {} });
  const rendered = call(engine, 'compile', texts[mainFile], 'Template Test');
  return rendered;
}

// Validate HTML: doctype, title tag, html tag, and the escaped name in content
// (the title, or for a template without one, such as the CV, the author).
function validateHtml(html) {
  assert.match(html.toLowerCase(), /^<!doctype html/);
  assert.match(html, /<title>/);
  assert.match(html, /<\/html>/);
  const escapedTitle = (html.includes('A &amp; B') ? testTitle : testAuthor).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  assert.match(html, new RegExp(escapedTitle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
}

const failures = [];

// Discover templates: web/src/templates/<id>/<format>/main.*
const templatesRoot = new URL('../../src/templates', import.meta.url).pathname;
const ids = (await readdir(templatesRoot, { withFileTypes: true }))
  .filter(e => e.isDirectory())
  .map(e => e.name);

for (const id of ids) {
  const templateRoot = join(templatesRoot, id);
  const formatDirs = (await readdir(templateRoot, { withFileTypes: true }))
    .filter(e => e.isDirectory() && e.name !== 'node_modules' && !e.name.startsWith('.'))
    .map(e => e.name);

  for (const format of formatDirs) {
    const formatPath = join(templateRoot, format);
    let tempDir;
    try {
      // Read all template files.
      const files = await readTemplateFiles(formatPath);
      assert.ok(files.length > 0, `no files in ${id}/${format}`);

      // Fill placeholders with test values.
      const filled = fillTemplate(files, format, { title: testTitle, author: testAuthor });

      // Assert all placeholders were filled.
      for (const { path, text } of filled) {
        assert.ok(!text.includes('{{'), `unfilled placeholder in ${id}/${format}/${path}`);
      }

      // Create temp directory and write files.
      tempDir = await mkdtemp(join(tmpdir(), `librepaper-template-${id}-${format}-`));
      for (const { path, text } of filled) {
        const filePath = join(tempDir, path);
        const fileDir = filePath.slice(0, filePath.lastIndexOf('/'));
        if (fileDir !== tempDir) {
          await mkdir(fileDir, { recursive: true });
        }
        await writeFile(filePath, text);
      }

      // Render by format.
      if (format === 'typst') {
        if (spawnSync('typst', ['--version'], { stdio: 'ignore' }).error) {
          console.log(`templates-render: ${id}/${format} skipped (typst not on PATH)`);
        } else {
          try {
            await run('typst', ['compile', 'main.typ', 'out.pdf'], { cwd: tempDir });
            const pdfData = await readFile(join(tempDir, 'out.pdf'));
            assert.ok(pdfData.length > 0);
          } catch (err) {
            throw new Error((err.stderr || err.stdout || err.message).slice(-1500));
          }
        }
      } else if (format === 'latex') {
        if (spawnSync('latexmk', ['--version'], { stdio: 'ignore' }).error) {
          console.log(`templates-render: ${id}/${format} skipped (latexmk not on PATH)`);
        } else {
          try {
            await run('latexmk', ['-pdf', '-interaction=nonstopmode', '-halt-on-error', 'main.tex'], { cwd: tempDir });
            const pdfData = await readFile(join(tempDir, 'main.pdf'));
            assert.ok(pdfData.length > 0);
          } catch (err) {
            let output = err.stderr || err.stdout || '';
            if (!output) {
              try {
                const logContent = await readFile(join(tempDir, 'main.log'), 'utf8');
                output = logContent;
              } catch { }
            }
            throw new Error(output.slice(-1500));
          }
        }
      } else if (format === 'quarto') {
        if (spawnSync('quarto', ['--version'], { stdio: 'ignore' }).error) {
          console.log(`templates-render: ${id}/${format} skipped (quarto not on PATH)`);
        } else {
          try {
            await run('quarto', ['render', 'main.qmd', '--to', 'html', '--no-execute'], { cwd: tempDir, maxBuffer: 2e6 });
            const html = await readFile(join(tempDir, 'main.html'), 'utf8');
            assert.ok(html.includes('A &amp; B'));
          } catch (err) {
            throw new Error((err.stderr || err.stdout || err.message).slice(-1500));
          }
        }
      } else if (format === 'markdown') {
        try {
          const texts = {};
          for (const { path, text } of filled) {
            texts[path] = text;
          }
          const result = await renderMarkdown('main.md', texts);
          assert.equal(result.ok, true);
          assert.deepEqual(result.diagnostics, []);
        } catch (err) {
          throw err;
        }
      } else if (format === 'html') {
        try {
          const html = await readFile(join(tempDir, 'main.html'), 'utf8');
          validateHtml(html);
        } catch (err) {
          throw err;
        }
      }
    } catch (err) {
      failures.push(`${id}/${format}: ${err.message}`);
    } finally {
      if (tempDir) await rm(tempDir, { recursive: true, force: true });
    }
  }
}

// Report results.
if (failures.length > 0) {
  const message = failures.map((f, i) => `  ${i + 1}. ${f}`).join('\n');
  throw new Error(`templates-render: ${failures.length} template(s) failed:\n${message}`);
}
console.log(`templates-render: ${ids.length} template(s) with all formats rendered successfully`);
