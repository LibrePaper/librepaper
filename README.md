# LibrePaper

Create or upload an HTML, Markdown, Typst, LaTeX or Quarto project in the
browser, share a link to it, and collect comments and highlights in real time.

- Highlight passages, suggest edits, and comment on the text
- Multiple people can annotate simultaneously, with live updates
- Create, edit, review, share, and render projects in the browser
- Trivial to deploy: one static binary, on your laptop or on a small server
- Export annotations or a complete independent project copy from the CLI

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
each format, sharing and review, the CLI, and running a server of your own. Its
source is in [`site/`](site/), and `make site` builds it.

The ordinary CLI covers `login`, `logout`, `list`, and one-shot `export`.
`admin` operates a deployment, `local` operates the companion on your own
computer, and `agent mcp` serves a document's tools to an agent.

```sh
librepaper export DOCUMENT --format markdown --output comments.md
librepaper export DOCUMENT --project --output ./paper-copy
```

An exported project is a snapshot copy. Editing it does not update the hosted
document.

- [Getting started](https://librepaper.org/start.html)
- [Running a server](https://librepaper.org/host.html)
- [Privacy and the LaTeX mirror](https://librepaper.org/host.html#privacy-and-the-latex-mirror)
- [Building from source](https://librepaper.org/architecture.html#building-from-source)
