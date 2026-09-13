# LibrePaper

Publish an HTML, Markdown, Typst, LaTeX or Quarto document, share a link to
it, and collect comments and highlights in real time.

- Highlight passages, suggest edits, and comment on figures
- Multiple people can annotate simultaneously, with live updates
- Publish documents from the CLI; review and manage them in the browser
- Trivial to deploy: one static binary, on your laptop or on a small server
- Export annotations as Markdown, W3C JSON-LD, or a response to reviewers

![A document open in LibrePaper, with highlighted passages and the comments sidebar.](docs/images/commenting.png)

## Try it

The tutorial is a real LibrePaper document, open to anyone. Drag across a
sentence and a comment box opens where you released:

**[Learn LibrePaper with Markdown](https://app.librepaper.org/docs/learn-librepaper-with-markdown-t678fbd47v)**

Signing in gives you a private copy of it, and the same walkthrough written in
Typst, HTML, LaTeX and Quarto.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/LibrePaper/librepaper/main/deploy/install.sh | sh
```

Linux and macOS; Windows binaries are on the
[releases page](https://github.com/LibrePaper/librepaper/releases).

## Documentation

The manual lives at **[librepaper.org](https://librepaper.org)** — authoring in
each format, sharing and review, the CLI, and running a server of your own.

- [Getting started](https://librepaper.org/start/index.html)
- [Running a server](https://librepaper.org/host/index.html)
- [Privacy and the LaTeX mirror](https://librepaper.org/host/index.html#privacy-and-the-latex-mirror)
- [Building from source](https://librepaper.org/internals.html#building-from-source)
