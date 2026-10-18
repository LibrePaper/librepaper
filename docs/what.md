---
title: "What is LibrePaper"
---

LibrePaper is a free and open source platform for collaborative writing. Write
with colleagues, collect comments on a published document, or connect an LLM
agent to comment and edit.

![The LibrePaper editor shows LaTeX source on the left and the compiled paper on the right, with its figure, equation and table.](../images/editor-latex.png)

## Features

Most features work in any web browser. Running code, agents, Zotero and local backups need the [desktop app](desktop-app.html).

Writing:

- Live preview of Markdown, HTML, Typst and LaTeX
- Computational notebooks with Quarto and Calepin
- Templates, tools and multi-file projects
- Bibliographies from BibTeX and Zotero
- Track changes: accept or reject each edit

Working with people:

- Simultaneous editors, with named carets and personal undo
- Highlights and threaded comments, live for everyone
- Share with readers, commenters or editors, signed in with GitHub or Google
- Chat with collaborators and AI agents
- Project chat is kept with the project. The owner can clear it from the chat menu or the command palette.
- Named versions and track changes. Owners can permanently trim older history, and an owner's trim also deletes chat.

LLM agents:

- Bring your own agent: no new subscription
- Ask for changes in the sidebar chat
- Pre-specified tasks on a selected passage
- Reader, commenter or editor permissions
- LibrePaper never sees your keys or tokens
- Any commercial model or local weights, over MCP

Free and open:

- Everything is free: no gated features
- Open source server, reader and renderers
- Public host at librepaper.org: 50 MB of storage per user and 30 uploads per hour
- Self-host with one static binary and a Postgres database, on a laptop or a small server
- Export project source and assets as ordinary files from the CLI
- Back up every project in an account to local ZIP files with the desktop app

## Your own fonts

- Upload `.otf` or `.ttf` files into the project. A `fonts/` folder keeps them out of the way.
- Typst: name the family. Font files anywhere in the project are found.

  ```typst
  #set text(font: "Fira Sans")
  ```

- LaTeX: use `fontspec` and name the file. `fontspec` switches the project to XeLaTeX. Fonts are found by file name only, not by family name.

  ```latex
  \setmainfont{FiraSans-Regular.otf}[Path=fonts/, BoldFont=FiraSans-Bold.otf]
  ```

- `.woff` and `.woff2` files are for HTML. Typst and LaTeX cannot read them.
- Everyone who can open the project, read links included, downloads the font files. Check that the font licence allows that.
