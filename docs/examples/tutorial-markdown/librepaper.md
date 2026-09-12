---
bibliography: references.bib
bibliography-style: apa
---

# Learn LibrePaper with Markdown

![The LibrePaper icon](librepaper-icon.png)

*The LibrePaper icon, loaded as a relative project asset.*

Welcome to LibrePaper. Edit this sentence in the browser and watch the preview
update. Your Markdown source, comments, highlights, and checkpoints travel with
the document.

## Source and preview

This file is Markdown. LibrePaper renders it to HTML in the browser, and
the native command-line client uses the matching renderer. The server synchronizes and stores the source; it does not compile the document.

## A small scientific example

For a sample of size $n$, the standard error of a mean is

$$
SE = \frac{s}{\sqrt{n}}.
$$

A formula is a rule a machine can be made to follow, the point Ada Lovelace
made when she described the Analytical Engine weaving algebraic patterns
[@lovelace1843]. Type `@` in the editor to cite another entry from
`references.bib`.

[Read the rendering guide in this project](sections/rendering.md). Markdown has no include directive, so a normal link is its portable multi-file equivalent.

The icon above is a relative project asset. Change the caption, then make a
checkpoint so a collaborator can compare the edit with the previous version.

## What Markdown adds

Markdown keeps prose close to the source. It also supports fenced code, tables,
links, and inline math without a preamble:

```r
mean(c(2, 4, 6, 8))
```

Try changing the four values and leave a comment on this paragraph.

## Working with other clients

The browser editor is enough for writing and reading. Use `librepaper sync` to
keep a local folder synchronized, or use `librepaper publish` to publish from the terminal. Readers render the small Markdown document in their browser and need no local toolchain.

## References
