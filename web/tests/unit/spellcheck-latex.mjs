// Checks the LaTeX markup scanner for the spellchecker: which ranges are
// skipped (commands, math, environments, name arguments, verbatim text,
// comments) and which prose stays for the spellchecker to see.
import assert from "node:assert/strict";

const { latexSkips } = await import("../../src/lib/spellcheck/latex.js");

// The words the spellchecker still sees: the text with every skipped range
// blanked, split on anything that is not a letter.
function words(text) {
  const kept = text.split("");
  for (const [from, to] of latexSkips(text)) for (let i = from; i < to; i++) kept[i] = " ";
  return kept.join("").split(/[^\p{L}]+/u).filter(Boolean);
}

{
  // Plain prose is untouched.
  assert.deepEqual(words("The quick brown fox."), ["The", "quick", "brown", "fox"]);
}
{
  // The command goes; the braced text it wraps stays prose.
  assert.deepEqual(words("\\emph{word} here"), ["word", "here"]);
}
{
  // Starred sectioning commands are skipped, their heading text is kept.
  assert.deepEqual(words("\\section*{Introduction}"), ["Introduction"]);
}
{
  // A citation's keys and optional note are not words.
  assert.deepEqual(words("\\citep[p.~3]{smith2020,doe2019}"), []);
}
{
  // Reference and cross-reference keys are skipped.
  assert.deepEqual(words("See \\ref{fig:a} and \\cref{sec:intro}."), ["See", "and"]);
}
{
  // A label is skipped with its key, even between words.
  assert.deepEqual(words("A\\label{sec:x}B"), ["A", "B"]);
}
{
  // Inline, display, \( \) and \[ \] math are each skipped whole.
  assert.deepEqual(words("a $x$ b $$y$$ c \\(z\\) d \\[w\\] e"), ["a", "b", "c", "d", "e"]);
}
{
  // An escaped dollar is a currency sign, and it does not open math.
  assert.deepEqual(words("Costs \\$5 and $x$ end"), ["Costs", "and", "end"]);
}
{
  // A comment runs to the end of its line, and the next line is checked.
  assert.deepEqual(words("keep this % a comment\nnext line"), ["keep", "this", "next", "line"]);
}
{
  // An escaped percent is text, not a comment.
  assert.deepEqual(words("50\\% is real % hidden"), ["is", "real"]);
}
{
  // Equation environments are skipped through their matching end.
  assert.deepEqual(words("Before\n\\begin{equation}\nx = y\n\\end{equation}\nAfter"), ["Before", "After"]);
}
{
  // A starred align environment is skipped whole.
  assert.deepEqual(words("\\begin{align*}a&=b\\end{align*} Text"), ["Text"]);
}
{
  // An environment that is not skipped keeps its body as prose.
  assert.deepEqual(words("\\begin{itemize}\n\\item first\n\\end{itemize}"), ["first"]);
}
{
  // The figure options and the file name are skipped, and the surrounding text stays.
  assert.deepEqual(words("See \\includegraphics[width=0.5\\linewidth]{figs/plot.png} here"), ["See", "here"]);
}
{
  // The first argument of href is a URL; the link text stays prose.
  assert.deepEqual(words("\\href{https://x.org}{link text}"), ["link", "text"]);
}
{
  // hyperref's key is skipped, and its braced link text stays prose.
  assert.deepEqual(words("\\hyperref[sec:intro]{the introductin}"), ["the", "introductin"]);
}
{
  // nameref prints text derived from its label, so the label is all that is skipped.
  assert.deepEqual(words("\\nameref{sec:intro}"), []);
}
{
  // natbib's citetext prints its braced argument, so that text stays prose.
  assert.deepEqual(words("\\citetext{priv.~comm.}"), ["priv", "comm"]);
}
{
  // A verbatim span is skipped up to its closing delimiter.
  assert.deepEqual(words("Use \\verb|foo_bar| here"), ["Use", "here"]);
}
{
  // Package options and the package name are both skipped.
  assert.deepEqual(words("\\usepackage[utf8]{inputenc}\nHello"), ["Hello"]);
}
{
  // A definition's name, optional count and body are all skipped.
  assert.deepEqual(words("\\newcommand{\\R}[1]{\\mathbb{R}^{#1}} ok"), ["ok"]);
}
{
  // A figure environment's placement option is skipped, and its caption stays.
  assert.deepEqual(words("\\begin{figure}[htbp]\nCaption text\n\\end{figure}"), ["Caption", "text"]);
}
{
  // A bracket in prose, not directly after a command, is not an option.
  assert.deepEqual(words("see [1] for it"), ["see", "for", "it"]);
}
{
  // An \item label is prose, so its optional argument is kept.
  assert.deepEqual(words("\\item[Term] defined here"), ["Term", "defined", "here"]);
}
{
  // A sectioning command's optional short title is prose, as is its heading.
  assert.deepEqual(words("\\section[Short title]{Long title}"), ["Short", "title", "Long", "title"]);
}
{
  // An unclosed \cite{ ends at the blank line, so the next paragraph is checked.
  assert.deepEqual(words("\\cite{smith\n\nNext paragraph here"), ["Next", "paragraph", "here"]);
}
{
  // A group that spans one line break but no blank line still closes normally.
  assert.deepEqual(words("\\cite{smith,\n  doe} after"), ["after"]);
}
{
  // An unclosed inline dollar ends at the line end, so the next line is checked.
  assert.deepEqual(words("x $a\nnext line"), ["x", "next", "line"]);
}
{
  // An unclosed display dollar runs to the end of the text.
  assert.deepEqual(words("$$ abc\nmore"), []);
}
{
  // An unclosed verbatim environment runs to the end of the text.
  assert.deepEqual(words("\\begin{verbatim}\nx y"), []);
}
{
  // The empty string has nothing to skip.
  assert.deepEqual(latexSkips(""), []);
}
{
  // Malformed input never throws, and every range stays inside the text.
  const samples = [
    "\\", "$", "$$", "\\(", "\\[", "\\begin{", "\\begin{equation}", "\\begin",
    "\\verb", "\\verb|", "\\cite[", "{", "}", "[", "%", "\\\\", "\\$", "\\end{x}",
    "\\newcommand{", "a [b", "\\href{x}", "\\\n%", "\\verb\n|",
  ];
  for (const sample of samples) {
    for (const [from, to] of latexSkips(sample)) {
      assert.ok(0 <= from && from <= to && to <= sample.length, `bad range in ${JSON.stringify(sample)}`);
    }
  }
}

{
  // Blanks may separate a name argument from its command, so the key is skipped.
  assert.deepEqual(words("\\cite {smith2020} after"), ["after"]);
}
{
  // Blanks may separate an option group from the file name that follows it.
  assert.deepEqual(words("\\includegraphics[width=5cm] {plot.png} after"), ["after"]);
}
{
  // Blanks may separate \begin and \end from their environment names.
  assert.deepEqual(words("\\begin {equation} qwrtz \\end {equation} after"), ["after"]);
}
{
  // An ordinary command does not skip blanks, so the word after it stays prose.
  assert.deepEqual(words("\\emph {word}"), ["word"]);
}
{
  // A subfile's name is skipped whole, as an include's is.
  assert.deepEqual(words("\\subfile{chapters/b}"), []);
}
{
  // An unbraced \input name is skipped, and the word after it stays prose.
  assert.deepEqual(words("\\input chapters/b.tex more"), ["more"]);
}
{
  // A braced \input name is still skipped, with or without blanks before it.
  assert.deepEqual(words("\\input{a} after"), ["after"]);
  assert.deepEqual(words("\\input {chapters/a} after"), ["after"]);
}
{
  // A citation at the very end of the text does not throw, with or without blanks after it.
  assert.deepEqual(words("\\cite"), []);
  assert.deepEqual(words("\\cite "), []);
}
{
  // A long line of completed \verb spans is scanned in linear time, and its last word is kept.
  assert.deepEqual(words("\\verb|x| ".repeat(20000) + "word"), ["word"]);
}

console.log("spellcheck-latex: markup is skipped and prose is kept");
