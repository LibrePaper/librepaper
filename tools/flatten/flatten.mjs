// The lint and the generator for the published single crate.
//
//   tools/flatten/flatten lint            check the rules sub-crates follow
//   tools/flatten/flatten build [--out D] generate target/flat/ (or D)
//
// The workspace is the development layout; crates.io gets one `librepaper`
// package. `build` folds every sub-crate into a module of the facade, rewrites
// paths, and writes a manifest, so the result is a normal crate. `lint` keeps
// that rewrite purely syntactic. See docs/dev/specs/SPEC-split-crates.md.
//
// Workspace members come from `cargo metadata`; no crate is named here except
// the facade (`librepaper`) and the test-support crate (`librepaper-testing`).
import { spawnSync } from "node:child_process";
import {
  cpSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const FACADE = "librepaper";
const TESTING = "testing";
// What a crate directory holds that is not a staged input.
const NOT_INPUTS = new Set(["Cargo.toml", "Cargo.lock", "build.rs", "src", "tests", "target"]);
// Inputs that live at the repository root rather than inside a crate. The
// shell's dist stays at web/dist by design (its build.rs exports
// LIBREPAPER_SHELL_DIST); .sqlx and skills are here until a crate owns them.
const ROOT_INPUTS = [
  { from: "web/dist", to: "dist", required: true },
  { from: ".sqlx", to: ".sqlx" },
  { from: "skills", to: "skills" },
];
const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function die(message) {
  console.error(`flatten: ${message}`);
  process.exit(1);
}

// ---------------------------------------------------------------- workspace

function loadWorkspace() {
  const r = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 1 << 28,
  });
  if (r.status !== 0) die(`cargo metadata failed\n${r.stderr}`);
  const meta = JSON.parse(r.stdout);
  const all = meta.packages
    .filter((p) => meta.workspace_members.includes(p.id))
    .map((p) => ({
      name: p.name,
      dir: dirname(p.manifest_path),
      manifestPath: p.manifest_path,
      short: p.name.replace(/^librepaper-/, ""),
    }));
  const facade = all.find((m) => m.name === FACADE);
  if (!facade) die(`no workspace member named ${FACADE}`);
  const subs = all.filter((m) => m !== facade).sort((a, b) => a.name.localeCompare(b.name));
  for (const m of subs) {
    if (!m.name.startsWith("librepaper-")) die(`workspace member ${m.name} is not named librepaper-*`);
  }
  return { root: meta.workspace_root, targetDir: meta.target_directory, facade, subs };
}

function walk(dir) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(path));
    else out.push(path);
  }
  return out.sort();
}

function inputsOf(crate) {
  return readdirSync(crate.dir)
    .filter((name) => !NOT_INPUTS.has(name))
    .sort();
}

// --------------------------------------------------------------------- lint

// A line, with a comment-only line reported as such: the rules below ignore
// text that starts with `//`.
const isComment = (line) => line.trimStart().startsWith("//");

function lintRustFiles(ctx, rule, test, message) {
  for (const crate of ctx.subs) {
    for (const file of walk(join(crate.dir, "src")).filter((f) => f.endsWith(".rs"))) {
      readFileSync(file, "utf8")
        .split("\n")
        .forEach((line, i) => {
          if (!isComment(line) && test(line)) ctx.report(file, i + 1, rule, message);
        });
    }
  }
}

// 1. Siblings are referenced only as full paths.
function ruleSiblingPaths(ctx) {
  const alias = /^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+librepaper_\w+\s*(?:;|as\b)/;
  const extern = /\bextern\s+crate\s+librepaper_/;
  lintRustFiles(
    ctx,
    "sibling-paths",
    (line) => alias.test(line) || extern.test(line),
    "reference a sibling crate by its full path (librepaper_x::a::B), not by `use librepaper_x;`, `as` or `extern crate`",
  );
}

// 2. No inner attributes in a sub-crate lib.rs.
function ruleInnerAttributes(ctx) {
  for (const crate of ctx.subs) {
    const file = join(crate.dir, "src/lib.rs");
    if (!existsSync(file)) continue;
    readFileSync(file, "utf8")
      .split("\n")
      .forEach((line, i) => {
        if (/^\s*#!\[/.test(line)) {
          ctx.report(file, i + 1, "inner-attribute", "lib.rs has no inner attributes except `//!` docs; put them on items or the workspace");
        }
      });
  }
}

// 3. No `$crate`.
function ruleDollarCrate(ctx) {
  lintRustFiles(ctx, "dollar-crate", (line) => /\$crate\b/.test(line), "macro_rules! must not use `$crate`; spell the sibling path");
}

// 4. No package-identity env macros.
function rulePackageEnv(ctx) {
  lintRustFiles(
    ctx,
    "package-env",
    (line) => /\bCARGO_(?:PKG_NAME|PKG_VERSION|CRATE_NAME)\b/.test(line),
    "CARGO_PKG_NAME, CARGO_PKG_VERSION and CARGO_CRATE_NAME differ in the flat crate; use the facade's VERSION",
  );
}

// 5. Staged inputs do not collide across crates or with the facade's.
function ruleInputCollisions(ctx) {
  const seen = new Map();
  // While the repository root still has the input, no crate may stage the name.
  for (const { from, to } of ROOT_INPUTS) {
    if (existsSync(join(ctx.root, from))) seen.set(to, `the repository root (${from})`);
  }
  for (const crate of [ctx.facade, ...ctx.subs]) {
    for (const name of inputsOf(crate)) {
      const owner = seen.get(name);
      if (owner) {
        ctx.report(join(crate.dir, name), 1, "input-collision", `top-level entry ${name} is also provided by ${owner}`);
      } else {
        seen.set(name, crate.name);
      }
    }
  }
}

// 6. A sub-crate's short name is not a module of the facade or `testing`.
function ruleModuleNames(ctx) {
  const taken = new Set(
    readdirSync(join(ctx.facade.dir, "src")).map((name) => name.replace(/\.rs$/, "")),
  );
  taken.add(TESTING);
  for (const crate of ctx.subs) {
    if (crate.short === TESTING) continue;
    if (taken.has(crate.short)) {
      ctx.report(crate.manifestPath, 1, "module-name", `${crate.name} would flatten to module ${crate.short}, which the facade already has`);
    }
  }
}

// True when `crate::` occurs inside a string literal on this line. A small
// scanner, not a lexer: multi-line strings are not seen, which is the
// approximation the rule accepts.
function crateInString(line) {
  let inString = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (inString) {
      if (c === "\\") i++;
      else if (c === '"') inString = false;
      else if (line.startsWith("crate::", i)) return true;
    } else if (c === '"') {
      inString = true;
    } else if (c === "'" && line[i + 1] === '"' && line[i + 2] === "'") {
      i += 2;
    } else if (c === "/" && line[i + 1] === "/") {
      return false;
    }
  }
  return false;
}

// 7. No `crate::` inside a string literal.
function ruleCrateInStrings(ctx) {
  lintRustFiles(ctx, "crate-in-string", crateInString, "`crate::` inside a string literal would be rewritten; build the text another way");
}

// 8. Every rustc-env a sub-crate build script emits, the flat one emits too.
function ruleBuildScriptEnv(ctx) {
  const emitted = (text) => [...text.matchAll(/cargo:rustc-env=([A-Za-z0-9_]+)/g)].map((m) => m[1]);
  const flatText = readFileSync(join(repoRoot, "tools/flatten/build.rs"), "utf8");
  const flat = new Set(emitted(flatText));
  for (const crate of ctx.subs) {
    const file = join(crate.dir, "build.rs");
    if (!existsSync(file)) continue;
    readFileSync(file, "utf8")
      .split("\n")
      .forEach((line, i) => {
        for (const name of emitted(line)) {
          if (!flat.has(name)) {
            ctx.report(file, i + 1, "build-script-env", `emits rustc-env ${name}, which tools/flatten/build.rs does not`);
          }
        }
      });
  }
}

function lint(ws) {
  const findings = [];
  const ctx = {
    ...ws,
    report: (file, line, rule, message) => findings.push(`${relative(ws.root, file)}:${line}: ${rule}: ${message}`),
  };
  for (const rule of [
    ruleSiblingPaths,
    ruleInnerAttributes,
    ruleDollarCrate,
    rulePackageEnv,
    ruleInputCollisions,
    ruleModuleNames,
    ruleCrateInStrings,
    ruleBuildScriptEnv,
  ]) {
    rule(ctx);
  }
  for (const f of findings) console.error(f);
  if (findings.length) die(`${findings.length} lint finding(s)`);
  console.log(`flatten lint: ${ws.subs.length} sub-crate(s) clean`);
}

// --------------------------------------------------------------------- TOML

// A reader for the subset the manifests use: tables, arrays of tables,
// dotted and quoted keys, strings, booleans, numbers, arrays, inline tables,
// comments.
function parseToml(text, file) {
  let i = 0;
  const fail = (message) => {
    throw new Error(`${file}:${text.slice(0, i).split("\n").length}: ${message}`);
  };
  const sticky = (re) => {
    re.lastIndex = i;
    const m = re.exec(text);
    if (m) i += m[0].length;
    return m;
  };
  const ws = () => sticky(/[ \t]*/y);
  const skip = () => sticky(/(?:[ \t\r\n]|#[^\n]*)*/y);
  const expect = (s) => {
    if (!text.startsWith(s, i)) fail(`expected ${s}`);
    i += s.length;
  };
  const basicString = () => {
    if (text.startsWith('"""', i)) fail("multi-line strings are not supported");
    i++;
    let out = "";
    for (;;) {
      const c = text[i++];
      if (c === undefined || c === "\n") fail("unterminated string");
      if (c === '"') return out;
      if (c !== "\\") {
        out += c;
        continue;
      }
      const e = text[i++];
      const simple = { n: "\n", t: "\t", r: "\r", b: "\b", f: "\f", '"': '"', "\\": "\\" };
      if (e in simple) out += simple[e];
      else if (e === "u") {
        out += String.fromCodePoint(parseInt(text.slice(i, i + 4), 16));
        i += 4;
      } else fail(`bad escape \\${e}`);
    }
  };
  const literalString = () => {
    const end = text.indexOf("'", i + 1);
    if (end < 0) fail("unterminated string");
    const out = text.slice(i + 1, end);
    i = end + 1;
    return out;
  };
  const keyPart = () => {
    ws();
    if (text[i] === '"') return basicString();
    if (text[i] === "'") return literalString();
    const m = sticky(/[A-Za-z0-9_-]+/y);
    if (!m) fail("bad key");
    return m[0];
  };
  const keyPath = () => {
    const parts = [keyPart()];
    ws();
    while (text[i] === ".") {
      i++;
      parts.push(keyPart());
      ws();
    }
    return parts;
  };
  const child = (obj, key) => {
    if (obj[key] === undefined) obj[key] = {};
    const v = obj[key];
    return Array.isArray(v) ? v[v.length - 1] : v;
  };
  const setPath = (obj, path, value) => {
    let cur = obj;
    for (const part of path.slice(0, -1)) cur = child(cur, part);
    cur[path[path.length - 1]] = value;
  };
  const value = () => {
    ws();
    const c = text[i];
    if (c === '"') return basicString();
    if (c === "'") return literalString();
    if (c === "[") {
      i++;
      const out = [];
      for (;;) {
        skip();
        if (text[i] === "]") {
          i++;
          return out;
        }
        out.push(value());
        skip();
        if (text[i] === ",") i++;
        else if (text[i] !== "]") fail("expected , or ]");
      }
    }
    if (c === "{") {
      i++;
      const out = {};
      for (;;) {
        skip();
        if (text[i] === "}") {
          i++;
          return out;
        }
        const path = keyPath();
        ws();
        expect("=");
        setPath(out, path, value());
        skip();
        if (text[i] === ",") i++;
        else if (text[i] !== "}") fail("expected , or }");
      }
    }
    if (sticky(/true(?![\w-])/y)) return true;
    if (sticky(/false(?![\w-])/y)) return false;
    const n = sticky(/[+-]?\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?(?![\w-])/y);
    if (n) return Number(n[0].replaceAll("_", ""));
    return fail("unsupported value");
  };

  const root = {};
  let cur = root;
  for (;;) {
    skip();
    if (i >= text.length) return root;
    if (text[i] === "[") {
      const isArray = text[i + 1] === "[";
      i += isArray ? 2 : 1;
      const path = keyPath();
      ws();
      expect(isArray ? "]]" : "]");
      let obj = root;
      if (isArray) {
        for (const part of path.slice(0, -1)) obj = child(obj, part);
        const last = path[path.length - 1];
        obj[last] ??= [];
        cur = {};
        obj[last].push(cur);
      } else {
        for (const part of path) obj = child(obj, part);
        cur = obj;
      }
    } else {
      const path = keyPath();
      ws();
      expect("=");
      setPath(cur, path, value());
      ws();
      if (text[i] !== "\n" && text[i] !== "\r" && text[i] !== "#" && i < text.length) fail("unexpected text after value");
    }
  }
}

const isTable = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const bareKey = /^[A-Za-z0-9_-]+$/;
const tomlKey = (k) => (bareKey.test(k) ? k : k.includes("'") ? JSON.stringify(k) : `'${k}'`);
const tomlPath = (path) => path.map(tomlKey).join(".");

function inline(v) {
  if (Array.isArray(v)) return `[${v.map(inline).join(", ")}]`;
  if (isTable(v)) {
    const entries = Object.entries(v);
    return entries.length ? `{ ${entries.map(([k, x]) => `${tomlKey(k)} = ${inline(x)}`).join(", ")} }` : "{}";
  }
  return typeof v === "string" ? JSON.stringify(v) : String(v);
}

// Writes `obj` as the table at `path` and, below it, its sub-tables.
function emitTable(lines, path, obj) {
  const scalars = Object.entries(obj).filter(([, v]) => !isTable(v));
  const tables = Object.entries(obj).filter(([, v]) => isTable(v));
  if (path.length && (scalars.length || !tables.length)) {
    lines.push("", `[${tomlPath(path)}]`);
  }
  for (const [k, v] of scalars) lines.push(`${tomlKey(k)} = ${inline(v)}`);
  for (const [k, v] of tables) emitTable(lines, [...path, k], v);
}

function emitArrayOfTables(lines, name, tables) {
  for (const t of tables ?? []) {
    lines.push("", `[[${name}]]`);
    for (const [k, v] of Object.entries(t)) lines.push(`${tomlKey(k)} = ${inline(v)}`);
  }
}

function emitDependencies(lines, path, deps) {
  const names = Object.keys(deps).sort();
  if (!names.length) return;
  lines.push("", `[${tomlPath(path)}]`);
  for (const name of names) lines.push(`${tomlKey(name)} = ${inline(deps[name])}`);
}

// ------------------------------------------------------------- dependencies

const DEP_SECTIONS = ["dependencies", "dev-dependencies", "build-dependencies"];

// A dependency as the flat manifest should state it, or null when it is a
// sibling (a path dependency or a librepaper-* package) and so is not a
// dependency of the flat crate at all.
function resolveDep(name, spec, wsDeps, where) {
  if (typeof spec === "string") return spec;
  if (spec.path !== undefined || (spec.package ?? name).startsWith("librepaper-")) return null;
  let out = spec;
  if (spec.workspace) {
    const base = wsDeps[name];
    if (base === undefined) throw new Error(`${where}: ${name} is not in [workspace.dependencies]`);
    const baseTable = typeof base === "string" ? { version: base } : base;
    if (baseTable.path !== undefined) return null;
    out = { ...baseTable };
    const features = [...(baseTable.features ?? []), ...(spec.features ?? [])];
    if (features.length) out.features = [...new Set(features)];
    if (spec.optional) out.optional = true;
  }
  const keys = Object.keys(out);
  return keys.length === 1 && keys[0] === "version" ? out.version : out;
}

// Two members depend on the same crate: features add up, anything else must
// agree, and a dependency is optional only if every member says so.
function mergeDep(name, a, b) {
  const x = typeof a === "string" ? { version: a } : a;
  const y = typeof b === "string" ? { version: b } : b;
  const out = { ...x };
  const features = [...new Set([...(x.features ?? []), ...(y.features ?? [])])];
  if (features.length) out.features = features;
  if (!x.optional || !y.optional) delete out.optional;
  for (const key of new Set([...Object.keys(x), ...Object.keys(y)])) {
    if (key === "features" || key === "optional") continue;
    if (JSON.stringify(x[key]) !== JSON.stringify(y[key])) {
      throw new Error(`members disagree about ${name}: ${key}`);
    }
  }
  const keys = Object.keys(out);
  return keys.length === 1 && keys[0] === "version" ? out.version : out;
}

function addDeps(into, deps, wsDeps, where) {
  for (const [name, spec] of Object.entries(deps ?? {})) {
    const resolved = resolveDep(name, spec, wsDeps, where);
    if (resolved === null) continue;
    into[name] = name in into ? mergeDep(name, into[name], resolved) : resolved;
  }
}

function unionDependencies(members, wsDeps) {
  const acc = { dependencies: {}, "dev-dependencies": {}, "build-dependencies": {}, target: {} };
  const sectionFor = (member, section) => (member.isTesting && section === "dependencies" ? "dev-dependencies" : section);
  for (const member of members) {
    const m = member.manifest;
    for (const section of DEP_SECTIONS) {
      addDeps(acc[sectionFor(member, section)], m[section], wsDeps, member.manifestPath);
    }
    for (const [cfg, table] of Object.entries(m.target ?? {})) {
      acc.target[cfg] ??= { dependencies: {}, "dev-dependencies": {}, "build-dependencies": {} };
      for (const section of DEP_SECTIONS) {
        addDeps(acc.target[cfg][sectionFor(member, section)], table[section], wsDeps, member.manifestPath);
      }
    }
  }
  // A dev-dependency identical to the normal one adds nothing.
  for (const [name, spec] of Object.entries(acc["dev-dependencies"])) {
    if (JSON.stringify(acc.dependencies[name]) === JSON.stringify(spec)) delete acc["dev-dependencies"][name];
  }
  return acc;
}

// ----------------------------------------------------------------- rewriting

function rewriteFlat(text, shorts) {
  return text.replace(/\blibrepaper_(\w+)::/g, (whole, name) => (shorts.has(name) ? `crate::${name}::` : whole));
}

// `crate::` becomes `crate::<short>::` first, through a placeholder so the
// `crate::<x>::` produced from `librepaper_<x>::` is not prefixed again.
function rewriteSub(text, short, shorts) {
  const placeholder = "__flatcrate__::";
  const marked = text.replace(/\bcrate::/g, `${placeholder}${short}::`);
  return rewriteFlat(marked, shorts).replaceAll(placeholder, "crate::");
}

// Copies a directory tree, passing `.rs` files through `transform`.
function copyTree(from, to, transform, rename = {}) {
  for (const file of walk(from)) {
    const rel = relative(from, file);
    const dest = join(to, rename[rel] ?? rel);
    mkdirSync(dirname(dest), { recursive: true });
    if (file.endsWith(".rs")) writeFileSync(dest, transform(readFileSync(file, "utf8")));
    else copyFileSync(file, dest);
  }
}

// ------------------------------------------------------------------- build

function stageSources(ws, out, shorts) {
  const { facade, subs } = ws;
  copyTree(join(facade.dir, "src"), join(out, "src"), (t) => rewriteFlat(t, shorts));
  if (existsSync(join(facade.dir, "tests"))) {
    copyTree(join(facade.dir, "tests"), join(out, "tests"), (t) => rewriteFlat(t, shorts));
  }
  for (const crate of subs) {
    copyTree(join(crate.dir, "src"), join(out, "src", crate.short), (t) => rewriteSub(t, crate.short, shorts), {
      "lib.rs": "mod.rs",
    });
  }
  const mods = subs.filter((c) => c.short !== TESTING).map((c) => `mod ${c.short};`);
  if (subs.some((c) => c.short === TESTING)) mods.push("#[cfg(test)]", `mod ${TESTING};`);
  const lib = join(out, "src/lib.rs");
  const note = "// Generated by tools/flatten/flatten: the former sub-crates, as private modules.";
  writeFileSync(lib, `${readFileSync(lib, "utf8").trimEnd()}\n\n${note}\n${mods.join("\n")}\n`);
}

// Each crate's non-source inputs, at the same crate-relative path in the root.
function stageInputs(ws, out) {
  const staged = [];
  for (const crate of [ws.facade, ...ws.subs]) {
    for (const name of inputsOf(crate)) {
      if (existsSync(join(out, name))) die(`input ${name} of ${crate.name} collides with another crate's`);
      cpSync(join(crate.dir, name), join(out, name), { recursive: true });
      staged.push(name);
    }
  }
  return staged;
}

function manifestText(ws, out, staged, manifests) {
  const root = manifests.root;
  const wsPackage = root.workspace?.package ?? {};
  const wsDeps = root.workspace?.dependencies ?? {};
  const facade = manifests.facade;

  const pkg = {};
  for (const [key, value] of Object.entries(facade.package)) {
    if (key === "metadata") continue;
    if (isTable(value) && value.workspace === true) {
      if (wsPackage[key] === undefined) throw new Error(`[workspace.package] has no ${key}`);
      pkg[key] = wsPackage[key];
    } else {
      pkg[key] = value;
    }
  }
  pkg.readme = "README.md";
  pkg.build = "build.rs";
  const entries = [...staged];
  for (const extra of ["assets.lock"]) if (!entries.includes(extra)) entries.push(extra);
  pkg.include = [
    "Cargo.toml",
    "build.rs",
    "README.md",
    "LICENSE",
    "src/**",
    "tests/**",
    ...entries.map((name) => (statSync(join(out, name)).isDirectory() ? `${name}/**` : name)),
  ];

  const lines = [
    "# Generated by tools/flatten/flatten from the workspace. Do not edit.",
    "# The repository builds as a workspace of internal sub-crates; this single",
    "# crate is what gets published. See docs/releasing.md.",
    "",
    "[package]",
  ];
  for (const [k, v] of Object.entries(pkg)) lines.push(`${tomlKey(k)} = ${inline(v)}`);
  if (facade.lib) emitTable(lines, ["lib"], facade.lib);
  emitArrayOfTables(lines, "bin", facade.bin);
  emitArrayOfTables(lines, "test", facade.test);

  const members = [
    { manifest: facade, manifestPath: ws.facade.manifestPath },
    ...ws.subs.map((c) => ({ manifest: manifests.subs[c.name], manifestPath: c.manifestPath, isTesting: c.short === TESTING })),
  ];
  const acc = unionDependencies(members, wsDeps);
  for (const section of DEP_SECTIONS) emitDependencies(lines, [section], acc[section]);
  for (const cfg of Object.keys(acc.target).sort()) {
    for (const section of DEP_SECTIONS) emitDependencies(lines, ["target", cfg, section], acc.target[cfg][section]);
  }

  // Keeps Cargo from looking upward: target/flat sits inside the repository,
  // whose workspace does not list it.
  lines.push("", "[workspace]");
  if (root.profile) emitTable(lines, ["profile"], root.profile);
  return `${lines.join("\n")}\n`;
}

// Everything but the SQLX_OFFLINE_DIR line and the comment above it: the flat
// crate has `.sqlx` at its own root, where sqlx looks by default.
function stripSqlxDir(text) {
  const lines = text.split("\n");
  const at = lines.findIndex((l) => /^\s*SQLX_OFFLINE_DIR\b/.test(l));
  if (at < 0) return text;
  let from = at;
  while (from > 0 && lines[from - 1].startsWith("#")) from--;
  if (from > 0 && lines[from - 1] === "") from--;
  lines.splice(from, at - from + 1);
  return lines.join("\n");
}

function splitLock(text) {
  const [header, ...blocks] = text.split(/\n(?=\[\[package\]\])/);
  const map = new Map();
  for (const block of blocks) {
    const name = /^name = "([^"]*)"/m.exec(block)?.[1];
    const version = /^version = "([^"]*)"/m.exec(block)?.[1];
    map.set(`${name} ${version}`, { name, text: block.trimEnd() });
  }
  return { header: header.trimEnd(), map };
}

// Lets Cargo prune the workspace-only entries, then insists that is all it did.
function pruneLockfile(ws, out) {
  const original = readFileSync(join(ws.root, "Cargo.lock"), "utf8");
  copyFileSync(join(ws.root, "Cargo.lock"), join(out, "Cargo.lock"));
  const r = spawnSync("cargo", ["metadata", "--offline", "--format-version", "1"], {
    cwd: out,
    stdio: ["ignore", "ignore", "inherit"],
  });
  if (r.status !== 0) die("cargo metadata failed in the flat crate");
  const before = splitLock(original);
  const after = splitLock(readFileSync(join(out, "Cargo.lock"), "utf8"));
  const problems = [];
  if (before.header !== after.header) problems.push("the lockfile header changed");
  for (const [key, entry] of before.map) {
    const now = after.map.get(key);
    if (!now) {
      if (!entry.name.startsWith("librepaper-")) problems.push(`package ${key} was removed`);
    } else if (now.text !== entry.text && entry.name !== FACADE) {
      problems.push(`package ${key} changed`);
    }
  }
  for (const key of after.map.keys()) if (!before.map.has(key)) problems.push(`package ${key} was added`);
  if (problems.length) die(`Cargo.lock changed beyond pruning librepaper-* and the facade's dependency list:\n  ${problems.join("\n  ")}`);
}

function build(ws, out) {
  if (out === ws.root || ws.root.startsWith(`${out}/`)) die(`refusing to delete ${out}`);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });

  const shorts = new Set(ws.subs.map((c) => c.short));
  const read = (path) => parseToml(readFileSync(path, "utf8"), relative(ws.root, path));
  const manifests = {
    root: read(join(ws.root, "Cargo.toml")),
    facade: read(ws.facade.manifestPath),
    subs: Object.fromEntries(ws.subs.map((c) => [c.name, read(c.manifestPath)])),
  };

  stageSources(ws, out, shorts);
  const staged = stageInputs(ws, out);
  for (const { from, to, required } of ROOT_INPUTS) {
    const src = join(ws.root, from);
    if (!existsSync(src)) {
      if (required && !existsSync(join(out, to))) die(`${from} is missing; build the shell first (make web)`);
      continue;
    }
    if (existsSync(join(out, to))) die(`${to} is provided by a crate and by the repository root (${from}); remove the root copy`);
    cpSync(src, join(out, to), { recursive: true });
    staged.push(to);
  }
  for (const name of ["README.md", "LICENSE", "assets.lock"]) {
    if (!existsSync(join(ws.root, name))) die(`${name} is missing from the repository root`);
    copyFileSync(join(ws.root, name), join(out, name));
  }
  copyFileSync(join(repoRoot, "tools/flatten/build.rs"), join(out, "build.rs"));
  writeFileSync(join(out, "Cargo.toml"), manifestText(ws, out, staged, manifests));

  const config = join(ws.root, ".cargo/config.toml");
  if (existsSync(config)) {
    mkdirSync(join(out, ".cargo"), { recursive: true });
    writeFileSync(join(out, ".cargo/config.toml"), stripSqlxDir(readFileSync(config, "utf8")));
  }
  pruneLockfile(ws, out);
  console.log(`flatten: wrote ${out} (${ws.subs.length} sub-crate(s))`);
}

// -------------------------------------------------------------------- main

const args = process.argv.slice(2);
let command = "build";
let outArg = null;
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--out") outArg = args[++i];
  else if (args[i] === "-h" || args[i] === "--help") {
    console.log("usage: tools/flatten/flatten [lint | build [--out DIR]]");
    process.exit(0);
  } else command = args[i];
}
if (!["lint", "build"].includes(command)) die(`unknown command ${command}`);
if (outArg === undefined) die("--out needs a directory");

const workspace = loadWorkspace();
if (command === "lint") lint(workspace);
else build(workspace, outArg ? resolve(outArg) : join(workspace.targetDir, "flat"));
