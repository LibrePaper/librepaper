#!/usr/bin/env node
// Download data only. No upstream build scripts or package installation run.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { CACHE, CASES, SOURCES, digest, filesIn } from "./corpus.mjs";

mkdirSync(CACHE, { recursive: true });
const roots = {};
for (const [name, source] of Object.entries(SOURCES)) {
  const archive = join(CACHE, `${name}.tar.gz`);
  if (!existsSync(archive)) {
    const repo = source.repository.replace("https://github.com/", "");
    const response = await fetch(`https://codeload.github.com/${repo}/tar.gz/${source.revision}`, { signal: AbortSignal.timeout(120000) });
    if (!response.ok) throw new Error(`${name}: HTTP ${response.status}`);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (digest(bytes) !== source.sha256) throw new Error(`${name}: archive checksum mismatch`);
    writeFileSync(archive, bytes);
  }
  if (digest(readFileSync(archive)) !== source.sha256) throw new Error(`${name}: cached archive checksum mismatch`);
  const directory = join(CACHE, `${name}-${source.revision}`);
  rmSync(directory, { recursive: true, force: true });
  mkdirSync(directory, { recursive: true });
  execFileSync("tar", ["-xzf", archive, "--strip-components=1", "-C", directory]);
  roots[name] = directory;
  console.log(`${name}: verified ${source.revision}`);
}

// ACM distributes documented sources. Use its docstrip recipes to extract the
// class and sample documents, just as a native TeX installation does.
for (const [directory, input] of [[roots.acmart, "acmart.ins"], [join(roots.acmart, "samples"), "samples.ins"]]) {
  execFileSync("latex", ["-no-shell-escape", "-interaction=nonstopmode", "-halt-on-error", input], {
    cwd: directory, stdio: "pipe", timeout: 30000,
    env: { ...process.env, openin_any: "p", openout_any: "p" },
  });
}

for (const example of CASES.filter((c) => c.source)) {
  const target = join(CACHE, "projects", example.id);
  rmSync(target, { recursive: true, force: true });
  mkdirSync(target, { recursive: true });
  const copy = (root, path, as = path) => {
    const destination = join(target, as);
    mkdirSync(join(destination, ".."), { recursive: true });
    cpSync(join(root, path), destination);
  };
  if (example.source === "acmart") {
    for (const path of filesIn(roots.acmart)) {
      if (/^[^/]+\.(cls|bst|bbx|cbx|dbx)$/.test(path) || path === "LICENSE") copy(roots.acmart, path);
      else if (path === `samples/${example.main}` || /^samples\/[^/]+\.(bib|pdf|png)$/.test(path)) copy(roots.acmart, path, path.slice(8));
    }
  } else if (example.source === "thesis") {
    for (const path of filesIn(roots.thesis)) {
      if ([example.main, "commands.tex", "abbreviations.tex", "bibliography.bib", "LICENSE"].includes(path)
          || /^[^/]+\.sty$/.test(path) || /^(figures|data)\//.test(path)) copy(roots.thesis, path);
    }
  } else {
    copy(roots.biblatex, `doc/latex/biblatex/examples/${example.main}`, example.main);
    copy(roots.biblatex, "bibtex/bib/biblatex/biblatex-examples.bib", "biblatex-examples.bib");
    copy(roots.biblatex, "README.md", "UPSTREAM-README.md");
  }
  console.log(`${example.id}: ${filesIn(target).length} files prepared`);
}
