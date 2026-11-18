---
title: "Bibliography"
---

Every format reads its references from a BibTeX file, usually `references.bib`. LaTeX, Typst, Markdown, Quarto and Org can all cite from the same file, so one `.bib` serves a whole project.

## Add references

- Your own file: upload a `.bib` exported from a reference manager, or type entries in the editor.
- DOI: in **Insert > Citation**, choose DOI and paste a DOI or a `doi.org` link. The entry is fetched from doi.org and added to your `.bib`.
- Zotero: in the [desktop app](desktop-app.html), choose Zotero to search your library. Zotero must be running, with "Allow other applications on this computer to communicate with Zotero" turned on in its Advanced settings.

A DOI or a Zotero item already in the `.bib` is reused, not added twice.

## Cite

**Insert > Citation** lists your references, one per line. Tick one or more and choose Insert. The dialog can be resized and remembers the last source you used.

You can also type the citation: `\cite{` in LaTeX, or `@` in the other formats, then pick a key from the completion list. A key that is missing from the bibliography is underlined. Hover over a key to see its entry.

## Where the bibliography is declared

| Format | Declaration |
|---|---|
| LaTeX | `\addbibresource{references.bib}` or `\bibliography{references}` |
| Typst | `#bibliography("references.bib")` |
| Markdown, Quarto | `bibliography: references.bib` in the front matter |
| Org | `#+BIBLIOGRAPHY: references.bib` |

If a document has no declaration, the first DOI or Zotero import adds one. In a Markdown or Quarto project with several files, a chapter uses the bibliography declared in its own front matter, then in the main file, then (Quarto) in `_quarto.yml`.

## Agents

[Agents](agents.html) add references with the `reference_from_doi` tool, which returns the entry as published rather than one the agent writes itself.
