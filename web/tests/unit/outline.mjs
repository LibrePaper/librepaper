import assert from "node:assert/strict";
import { extractOutline } from "../../src/lib/outline.js";

const headings = (source, format) => extractOutline(source, format).map(({ title, level, from, line }) => ({ title, level, from, line }));

{
  const source = "---\ntitle: Draft\n---\r\n# First 😀 ##\r\ntext\r\nSetext\r\n------\r\n<!-- # hidden -->\r\n```md\r\n# fenced\r\n```\r\n    # indented\r\n## Second\r\n";
  assert.deepEqual(headings(source, "markdown"), [
    { title: "First 😀", level: 1, from: source.indexOf("# First"), line: 4 },
    { title: "Setext", level: 2, from: source.indexOf("Setext"), line: 6 },
    { title: "Second", level: 2, from: source.indexOf("## Second"), line: 13 },
  ]);
  assert.deepEqual(extractOutline("# x", "quarto"), [{ title: "x", level: 1, from: 0, line: 1 }]);
  assert.deepEqual(extractOutline("# x", "unknown"), []);
}

{
  const source = "text\r\n= Top\r\n#let raw = ```\r\n= ignored\r\n```\r\n/*\r\n= hidden\r\n*/\r\n#heading(level: 2)[Generated]\r\n= Bottom\r\n";
  assert.deepEqual(headings(source, "typst"), [
    { title: "Top", level: 1, from: 6, line: 2 },
    { title: "Generated", level: 2, from: source.indexOf("#heading"), line: 9 },
    { title: "Bottom", level: 1, from: source.indexOf("= Bottom"), line: 10 },
  ]);
  assert.deepEqual(headings("// ```\n= Visible\n`#heading[Fake]`\n#let x = \"#heading[Also fake]\"", "typst"), [
    { title: "Visible", level: 1, from: 7, line: 2 },
  ]);
}

{
  const source = '<!-- <h1>hidden</h1> -->\n<script>\n<h2>hidden</h2>\n</script>\n<pre>\n<h2>also hidden</h2>\n</pre>\n<h1 class="a>still attr">Hello <em>world</em> &amp; all</h1>\n<h3>Next</h3>';
  assert.deepEqual(headings(source, "html"), [
    { title: "Hello world & all", level: 1, from: source.lastIndexOf("<h1"), line: 8 },
    { title: "Next", level: 3, from: source.lastIndexOf("<h3"), line: 9 },
  ]);
  assert.deepEqual(headings('<h1>Bad &#99999999;</h1>', "html"), [{ title: "Bad &#99999999;", level: 1, from: 0, line: 1 }]);
}

{
  const source = String.raw`% \section{hidden}
\part{Part}
\chapter*{Chapter}
\section[Short title]{Long
  title}
\begin{verbatim}
\subsection{hidden}
\end{verbatim}
\subsection*{Methods}
\paragraph{Fine detail}`;
  assert.deepEqual(headings(source, "latex"), [
    { title: "Part", level: 1, from: source.indexOf("\\part"), line: 2 },
    { title: "Chapter", level: 2, from: source.indexOf("\\chapter"), line: 3 },
    { title: "Long title", level: 3, from: source.indexOf("\\section["), line: 4 },
    { title: "Methods", level: 4, from: source.indexOf("\\subsection*"), line: 9 },
    { title: "Fine detail", level: 6, from: source.indexOf("\\paragraph"), line: 10 },
  ]);
}

const unicode = "😀\r\n# Heading";
assert.equal(extractOutline(unicode, "markdown")[0].from, unicode.indexOf("#"));
assert.equal(extractOutline(unicode, "markdown")[0].line, 2);
const reviewCases = [
 ['html', '<h1>A<em title="x > y">B</em>C<br>D</h1>', ['ABC D']],
 ['html', '<h1 title="x > y">Good</h1>', ['Good']],
 ['html', '<div title="<h1>Fake</h1>"></div><h2>Real</h2>', ['Real']],
 ['html', '<textarea><h1>Fake</h1></textarea><h1>Real</h1>', ['Real']],
 ['html', '<h1>&#9999999999; heading</h1>', null],
 ['markdown', '<!--\n```\n-->\n# Real', ['Real']],
 ['markdown', '```\n# Fake\n```suffix\n# Still fake\n```\n# Real', ['Real']],
 ['markdown', 'Title\n---\n---', ['Title']],
 ['markdown', '<script>\n# Fake\n</script>\n\n# Real', ['Real']],
 ['typst', '`#heading[Fake]`\n= Real', ['Real']],
 ['typst', '#let x = "#heading[Fake]"\n= Real', ['Real']],
 ['typst', '/* ``` */\n= Real', ['Real']],
 ['typst', '#heading(level: 2, numbering: none)[Real]', ['Real']],
 ['latex', String.raw`\section% hi
{Real}`, ['Real']],
 ['latex', String.raw`\begin{verbatim*}
\section{Fake}
\end{verbatim*}
\section{Real}`, ['Real']],
];
for (const [format, source, expected] of reviewCases) {
  const actual = extractOutline(source, format).map(heading => heading.title);
  if (expected !== null) assert.deepEqual(actual, expected, format + ': ' + source);
}

console.log("outline: markdown/quarto, typst, html, and latex extraction passed");
