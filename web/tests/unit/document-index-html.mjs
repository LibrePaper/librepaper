import assert from "node:assert/strict";
import test from "node:test";
import { htmlAdapter } from "../../src/lib/document-index/adapters/html.js";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import { diagnostics } from "../../src/lib/document-index/queries.js";
import { extractOutline } from "../../src/lib/outline.js";
import { skipRanges } from "../../src/lib/spellcheck/index.js";

/** The items of one kind, in source order. */
const ofKind = (items, kind) => items.filter((item) => item.kind === kind);

/** The source text a range covers. */
const span = (text, [from, to]) => text.slice(from, to);

const PAGE_ONE = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Field notes</title>
  <link rel="stylesheet" href="styles/site.css?v=3">
  <style>
    h2 { color: #123; }
  </style>
  <script src="js/app.js"></script>
</head>
<body>
  <!-- <h2 id="hidden">Not a heading</h2> -->
  <header>
    <h1 id="top">Field notes</h1>
  </header>
  <main>
    <section id="intro">
      <h2 id="sec-intro">Introduction</h2>
      <p>See <a href="#fig-plot">the plot</a> and <a href="notes.html#top">the notes</a>.</p>
      <figure id="fig-plot">
        <img src="images/plot.png?w=640" alt="Plot">
        <figcaption>Growth <em>by</em> year</figcaption>
      </figure>
    </section>
    <section>
      <h3>Methods &amp; data</h3>
      <pre><code>x &lt;- 1</code></pre>
      <ul>
        <li><a href="https://example.org/a">external</a></li>
      </ul>
    </section>
  </main>
  <script>
    document.write("<h2 id='fake'>x</h2>");
  </script>
</body>
</html>
`;

const PAGE_TWO = `<!DOCTYPE html>
<html>
<body>
<div class="page" id=page>
  <nav><ul><li><a href=#install>Install</a></li></ul></nav>
  <article>
    <h2 id='install'>Install <code>lp</code></h2>
    <div class="note">
      <p>Run <code>make</code>.</p>
      <div class="inner">
        <p>Nested &amp; deep.</p>
      </div>
    </div>
    <table>
      <tr><td>1</td></tr>
    </table>
    <details id="more">
      <summary>More</summary>
      <img src='a%20b.png'>
    </details>
  </article>
</div>
</body>
</html>
`;

test("headings match the outline and prose skips match the spellchecker", () => {
  for (const text of [PAGE_ONE, PAGE_TWO]) {
    const items = htmlAdapter(text, "page.html");
    const headings = ofKind(items, "heading");
    assert.deepEqual(
      headings.map((item) => [item.range[0], item.level, item.title]),
      extractOutline(text, "html").map((heading) => [heading.from, heading.level, heading.title]),
    );
    for (const heading of headings) {
      assert.match(span(text, heading.range), /<\/h[1-6]>$/i);
    }
    assert.deepEqual(ofKind(items, "markup").map((item) => item.range), skipRanges(text, "html"));
  }
});

test("a realistic page yields its labels, titles, refs, assets and includes", () => {
  const items = htmlAdapter(PAGE_ONE, "index.html");
  assert.deepEqual(
    ofKind(items, "label").map((item) => [item.name, item.title]),
    [
      ["top", "Field notes"],
      ["intro", undefined],
      ["sec-intro", "Introduction"],
      ["fig-plot", "Growth by year"],
    ],
  );
  assert.deepEqual(ofKind(items, "ref").map((item) => item.name), ["fig-plot"]);
  assert.deepEqual(
    ofKind(items, "link").map((item) => [item.name, item.attrs]),
    [
      ["notes.html", { file: "true", fragment: "top" }],
      ["https://example.org/a", { external: "true" }],
    ],
  );
  assert.deepEqual(ofKind(items, "include"), []);
  assert.deepEqual(
    ofKind(items, "asset").map((item) => item.name),
    ["styles/site.css", "js/app.js", "images/plot.png"],
  );
});

test("each label carries its target kind", () => {
  const text = [
    '<h2 id="s">Title</h2>',
    '<figure id="f"><img src="x.png" id="inside"></figure>',
    '<table id="t"></table>',
    '<pre id="p">code</pre>',
    '<div class="MathJax" id="m">x</div>',
    '<math id="mm"></math>',
    '<ul><li id="i">item</li></ul>',
    '<aside id="fn1" role="doc-footnote">note</aside>',
    '<section id="fn2" class="footnote other">note</section>',
    '<section id="sec">plain</section>',
    '<div id="d">x</div>',
  ].join("");
  const labels = ofKind(htmlAdapter(text, "x.html"), "label");
  assert.deepEqual(
    labels.map((item) => [item.name, item.attrs.target]),
    [
      ["s", "section"],
      ["f", "figure"],
      ["inside", "figure"],
      ["t", "table"],
      ["p", "listing"],
      ["m", "equation"],
      ["mm", "equation"],
      ["i", "item"],
      ["fn1", "footnote"],
      ["fn2", "footnote"],
      ["sec", "other"],
      ["d", "other"],
    ],
  );
});

test("a label on a code element inside no figure is a listing", () => {
  const text = '<p><code id="c">x</code></p>';
  const labels = ofKind(htmlAdapter(text, "x.html"), "label");
  assert.deepEqual(labels.map((item) => item.attrs.target), ["listing"]);
});

test("the head gives the title, authors, description and language", () => {
  const text = [
    '<!doctype html>',
    '<html lang="fr-CA">',
    "<head>",
    "<title>Notes &amp; <em>more</em></title>",
    '<meta name="author" content="Ada Lovelace">',
    '<meta name="Author" content="Alan &amp; Grace">',
    '<meta name="description" content="A short summary.">',
    '<meta name="date" content="2026-01-01">',
    "</head>",
    "<body><svg><title>icon</title></svg></body>",
    "</html>",
  ].join("\n");
  const items = ofKind(htmlAdapter(text, "x.html"), "metadata");
  assert.deepEqual(
    items.map((item) => [item.name, item.title]),
    [
      ["language", "fr-CA"],
      ["title", "Notes & more"],
      ["author", "Ada Lovelace"],
      ["author", "Alan & Grace"],
      ["abstract", "A short summary."],
    ],
  );
  const language = items.find((item) => item.name === "language");
  assert.equal(span(text, language.nameRange), "fr-CA");
  const title = items.find((item) => item.name === "title");
  assert.equal(span(text, title.range), "<title>Notes &amp; <em>more</em></title>");
});

test("a page with no head metadata gives no metadata items", () => {
  assert.deepEqual(ofKind(htmlAdapter("<p>x</p>", "x.html"), "metadata"), []);
});

test("ids are read in double, single and unquoted form", () => {
  const text = '<p id="a">one</p><p id=\'b\'>two</p><p id=c>three</p>';
  const labels = ofKind(htmlAdapter(text, "x.html"), "label");
  assert.deepEqual(
    labels.map((item) => [item.name, span(text, item.range), span(text, item.nameRange)]),
    [
      ["a", 'id="a"', "a"],
      ["b", "id='b'", "b"],
      ["c", "id=c", "c"],
    ],
  );
});

test("titles come from the heading on h1 to h6 and the figcaption of a figure", () => {
  const text = '<h2 id="s">Setup run</h2><figure id="f"><img src="x.png"><figcaption>Growth <em>by</em>  year &amp; rain</figcaption></figure><div id="d">x</div>';
  const labels = ofKind(htmlAdapter(text, "x.html"), "label");
  assert.deepEqual(
    labels.map((item) => [item.name, item.title]),
    [
      ["s", "Setup run"],
      ["f", "Growth by year & rain"],
      ["d", undefined],
    ],
  );
});

test("href fragments are refs and other hrefs are links", () => {
  const text = `<a href="#x">a</a><a href='#y'>b</a><a href=#z>c</a><a href="#">d</a><a href="other.html#q">e</a>`;
  const items = htmlAdapter(text, "x.html");
  assert.deepEqual(
    ofKind(items, "ref").map((item) => [
      item.name,
      item.namespace,
      span(text, item.range),
      span(text, item.nameRange),
    ]),
    [
      ["x", "label", 'href="#x"', "x"],
      ["y", "label", "href='#y'", "y"],
      ["z", "label", "href=#z", "z"],
    ],
  );
  assert.deepEqual(ofKind(items, "include"), []);
  assert.deepEqual(
    ofKind(items, "link").map((item) => [item.name, item.attrs, span(text, item.range)]),
    [
      ["other.html", { file: "true", fragment: "q" }, 'href="other.html#q"'],
    ],
  );
});

test("a link to a URL or a file is a link item with its name and fragment", () => {
  const text = [
    '<a href="https://e.org/p?q=1">e</a>',
    '<a href="mailto:a@b.c">m</a>',
    '<a href="//cdn.example.org/x">p</a>',
    '<a href="sections/a.html?x=1#top">f</a>',
    '<a href="guide.pdf">g</a>',
    '<a href="#sec">s</a>',
    '<a href="">empty</a>',
  ].join("");
  const links = ofKind(htmlAdapter(text, "x.html"), "link");
  assert.deepEqual(
    links.map((item) => [item.name, item.attrs, span(text, item.nameRange)]),
    [
      ["https://e.org/p?q=1", { external: "true" }, "https://e.org/p?q=1"],
      ["mailto:a@b.c", { external: "true" }, "mailto:a@b.c"],
      ["//cdn.example.org/x", { external: "true" }, "//cdn.example.org/x"],
      ["sections/a.html", { file: "true", fragment: "top" }, "sections/a.html"],
      ["guide.pdf", { file: "true" }, "guide.pdf"],
    ],
  );
});

test("local files are assets or includes; URLs and data URIs are neither", () => {
  const text = [
    '<img src="a.png?v=2#frag">',
    '<script src="https://cdn.example.org/x.js"></script>',
    '<img src="//cdn.example.org/x.png">',
    '<img src="data:image/png;base64,AAAA">',
    '<link rel="stylesheet" href="s/site.css">',
    '<source src="clip.mp4">',
    '<iframe src="#top"></iframe>',
    '<video src="v.mp4"></video>',
    '<audio src="sound.ogg"></audio>',
    '<link rel="icon" href="data:,">',
    '<iframe src="embed/map.html?z=2"></iframe>',
  ].join("");
  const items = htmlAdapter(text, "x.html");
  const assets = ofKind(items, "asset");
  assert.deepEqual(assets.map((item) => item.name), ["a.png", "s/site.css", "clip.mp4", "v.mp4", "sound.ogg"]);
  assert.deepEqual(assets.map((item) => span(text, item.nameRange)), assets.map((item) => item.name));
  assert.deepEqual(ofKind(items, "include").map((item) => item.name), ["embed/map.html"]);
  assert.deepEqual(ofKind(items, "link"), []);
});

test("a link to a missing project page is a missing-file diagnostic", () => {
  const text = '<a href="gone.html#x">g</a>';
  const project = createProjectIndex();
  project.setFiles(["index.html"]);
  project.update({ path: "index.html", format: "html", length: text.length, items: htmlAdapter(text, "index.html") });
  assert.deepEqual(
    diagnostics(project, "index.html").map((item) => [item.code, item.name]),
    [["missing-file", "gone.html"]],
  );
});

test("comments and script or style bodies are never read as markup", () => {
  const text = [
    '<!-- <p id="c1"><a href="#gone">x</a></p> -->',
    "<script>var s = '<p id=\"s1\">';</script>",
    '<style>a[href="#css"] { }</style>',
    '<p id="real">ok</p>',
  ].join("");
  const items = htmlAdapter(text, "x.html");
  assert.deepEqual(ofKind(items, "label").map((item) => item.name), ["real"]);
  assert.deepEqual(ofKind(items, "ref"), []);
  assert.deepEqual(ofKind(items, "code").map((item) => span(text, item.range)), [
    "<script>var s = '<p id=\"s1\">';</script>",
    '<style>a[href="#css"] { }</style>',
  ]);
});

test("nested multi-line elements give one block each, pairing by name", () => {
  const text = [
    '<div id="outer">',
    "  <div>",
    "    <div>x</div>",
    "  </div>",
    "</div>",
    "<section>one line</section>",
  ].join("\n");
  const blocks = ofKind(htmlAdapter(text, "x.html"), "block");
  assert.deepEqual(
    blocks.map((item) => [span(text, item.range), item.attrs.tag]),
    [
      ["\n  <div>\n    <div>x</div>\n  </div>\n", "div"],
      ["\n    <div>x</div>\n  ", "div"],
    ],
  );
});

test("pre and code elements are code items", () => {
  const text = "<p><code>a</code></p>\n<pre>\nb\n</pre>";
  const items = htmlAdapter(text, "x.html");
  assert.deepEqual(ofKind(items, "code").map((item) => span(text, item.range)), [
    "<code>a</code>",
    "<pre>\nb\n</pre>",
  ]);
  assert.deepEqual(ofKind(items, "block").map((item) => item.attrs.tag), ["pre"]);
});

test("a value that is not a string gives no items", () => {
  assert.deepEqual(htmlAdapter(null, "x.html"), []);
});

test("an unknown fragment is an unknown-label diagnostic; a missing image is missing-file", () => {
  const text = [
    '<h1 id="top">Title</h1>',
    '<p><a href="#missing">x</a> <a href="#top">top</a></p>',
    '<img src="missing.png">',
    '<img src="present.png">',
  ].join("\n");
  const project = createProjectIndex();
  project.setFiles(["index.html", "present.png"]);
  project.update({
    path: "index.html",
    format: "html",
    length: text.length,
    items: htmlAdapter(text, "index.html"),
  });
  assert.deepEqual(
    diagnostics(project, "index.html").map((item) => [item.code, item.name]),
    [
      ["unknown-label", "missing"],
      ["missing-file", "missing.png"],
    ],
  );
});

test("a fragment names an id on its own page only", () => {
  const one = '<h1 id="top">One</h1><a href="#top">up</a>';
  const two = '<a href="#top">up</a>';
  const project = createProjectIndex();
  project.setFiles(["one.html", "two.html"]);
  for (const [path, text] of [["one.html", one], ["two.html", two]]) {
    project.update({ path, format: "html", length: text.length, items: htmlAdapter(text, path) });
  }
  assert.deepEqual(diagnostics(project, "one.html"), []);
  assert.deepEqual(diagnostics(project, "two.html").map((item) => item.code), ["unknown-label"]);
});
