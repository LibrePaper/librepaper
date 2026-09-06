// The rules that keep the application looking like one application.
//
// A design system is only worth having if every screen is built from it, and
// nothing enforces that on its own: it takes one hurried page with a colour
// written into it for the drift to start. These are the three rules that
// matter, checked over the sources rather than trusted to memory.
//
// Run by `make test`, and by `bun run check`.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = new URL("..", import.meta.url).pathname;
const SRC = join(ROOT, "src");

// The theme is the one place a colour is written down, and the stylesheet
// beside it is the one place the application's own shapes are described.
const THEME = new Set(["src/styles/theme.css", "src/styles/komodoc.css", "src/styles/app.css"]);

// Two things are outside the design system, and have to be.
//
// The agent runs inside the document frame, on the documents origin, where
// none of this stylesheet reaches it: the colours it paints highlights in
// travel with it or they do not exist.
//
// The colours in collab.js are people rather than furniture. One is picked
// when somebody joins a session and sent to everyone else, so it has to be a
// value on the wire, and it has to stay legible against a document nobody
// here controls.
const OUTSIDE = [/^src\/agent\//, /^src\/lib\/collab\.js$/];

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

const problems = [];
const complain = (file, line, rule, text) =>
  problems.push(`${file}:${line}  ${rule}\n    ${text.trim().slice(0, 100)}`);

for (const path of files(SRC)) {
  const name = relative(ROOT, path);
  if (THEME.has(name) || OUTSIDE.some((each) => each.test(name))) continue;
  if (!/\.(svelte|js|css)$/.test(name)) continue;
  const source = readFileSync(path, "utf8");

  source.split("\n").forEach((line, index) => {
    const at = index + 1;
    // A comment explaining a colour is not a colour.
    const code = line.replace(/\/\/.*$/, "").replace(/\/\*.*?\*\//g, "");

    // 1. Colour comes from the theme. A hex or an rgb() in a component is a
    //    decision made twice, and the second one drifts.
    if (/#[0-9a-fA-F]{3,8}\b/.test(code) && !/["'`]#[a-z-]+["'`]/.test(code)) {
      complain(name, at, "a colour is written down; use a theme token", line);
    }
    if (/\b(rgb|hsl)a?\(/.test(code)) {
      complain(name, at, "a colour is written down; use a theme token", line);
    }

    // 2. Size comes from the scale. Tailwind's arbitrary values are the escape
    //    hatch, and an escape hatch used twice is a scale of its own.
    const arbitrary = code.match(/\b(?:p|m|gap|w|h|text|top|left|right|bottom)-\[[^\]]+\]/g);
    if (arbitrary) {
      complain(name, at, `an arbitrary size (${arbitrary[0]}); use the spacing or text scale`, line);
    }

    // 3. A control is a component. Skeleton's button classes belong in the
    //    components that wrap them, so a page cannot invent a fourth kind of
    //    button that is almost like the other three.
    if (/class="[^"]*\bbtn-icon\b/.test(code) && !/components\/(IconButton|Toasts)\.svelte$/.test(name)) {
      complain(name, at, "an icon button drawn by hand; use <IconButton>", line);
    }
  });
}

// 4. A global class of ours must not be a name Skeleton has taken. Its
//    utilities are plain class names -- `card`, `chip`, `mark` -- and a
//    stylesheet of ours using one of them means both rules apply to the same
//    element. That is how the komodo ended up drawn on a sage highlight: our
//    logo wore a class called `mark`, and so does Skeleton's marker pen.
const skeleton = new Set();
for (const path of files(join(ROOT, "node_modules/@skeletonlabs/skeleton/src"))) {
  if (!path.endsWith(".css")) continue;
  for (const found of readFileSync(path, "utf8").matchAll(/@utility\s+([\w-]+)/g)) skeleton.add(found[1]);
}
for (const path of files(join(SRC, "styles"))) {
  if (!path.endsWith(".css")) continue;
  const name = relative(ROOT, path);
  const source = readFileSync(path, "utf8");
  source.split("\n").forEach((line, index) => {
    for (const found of line.matchAll(/\.([a-zA-Z][\w-]*)/g)) {
      if (skeleton.has(found[1])) {
        complain(name, index + 1, `.${found[1]} is also a Skeleton utility; rename it or scope it to its component`, line);
      }
    }
  });
}

// 5. A prop must not be named after a rune. Svelte reads `$state` in a
//    component that has a prop called `state` as the store `state`, compiles
//    it to a subscription, and the component throws "subscribe is not a
//    function" on mount -- with no warning at build time and a blank page at
//    run time. The same is true of every other rune.
const RUNES = ["state", "derived", "props", "effect", "bindable", "inspect", "host"];
for (const path of files(SRC)) {
  const name = relative(ROOT, path);
  if (!name.endsWith(".svelte")) continue;
  const source = readFileSync(path, "utf8");
  const declaration = source.match(/let\s*\{([^}]*)\}\s*=\s*\$props\(\)/);
  if (!declaration) continue;
  const named = declaration[1].split(",").map((each) => each.split(/[:=]/)[0].trim());
  for (const rune of RUNES) {
    if (!named.includes(rune)) continue;
    const at = source.slice(0, declaration.index).split("\n").length;
    complain(name, at, `a prop called \`${rune}\` shadows the \`$${rune}\` rune; rename it`, declaration[0]);
  }
}

if (problems.length) {
  console.error(`The pages are drifting from the design system:\n\n${problems.join("\n\n")}\n`);
  process.exit(1);
}
console.log("vocabulary: every colour and size comes from the theme");
