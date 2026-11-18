---
title: "Security"
---

This is a weird world. A single developer can now build a massive code base for a complex product in a matter of weeks or months. The norms for what counts as "good" or "careful" development are shifting, and the open source community has not converged on a consensus approach yet.

For LibrePaper, I try to follow best practices: I run several rounds of automated and supervised review, using multiple frontier agents. But no single developer can read or assess every line of a code base this large, this recent, and moving this fast. That raises important security concerns.

For transparency, I should state that my primary audience for LibrePaper is:

- People who use edit files using the free hosted service at librepaper.org.
- People who host LibrePaper on a spare laptop, a Raspberry Pi or a $6-a-month virtual private server.

This is not as restrictive as it sounds, since the software is designed to serve hundreds of people on a machine with very limited resources. Small teams and labs can easily support dozens of simultaneous editors for "free". But the limited scope of these intended deployments does inherently limit the risks and blast radius somewhat.

In an institutional setting such as universities or large research centers, however, the risks can be greater. So I completely understand that some IT staffers might be more reluctant to deploy LibrePaper. I would be too! Unfortunately, I do not have the budget to hire a 3rd party who can conduct an independent audit of this free software, so a non-negligible amount of uncertainty is bound to remain.

That said, some features of the LibrePaper architecture, and some habits on the host's side, can drastically limit the risks, both for the client and the server.

## Open source and open for collaboration

LibrePaper is open source, under the MIT licence. Anyone can read the code, test it and help make it safer. I welcome that!

- **Vulnerabilities:** Email [admin@librepaper.org](mailto:admin@librepaper.org). Do not open a public issue.
- **Any other recommendation or question:** Open an [issue](https://github.com/LibrePaper/librepaper/issues) or start a [discussion](https://github.com/LibrePaper/librepaper/discussions) on GitHub.

## What the design limits

- **Documents are never compiled on the server.** LaTeX, Typst, Quarto and friends never run there. Typesetting runs as WebAssembly inside the browser's sandbox, which gives you some protection from a hostile document.
- **The server runs no AI model.** An agent reaches LibrePaper only through an API, with a token and a permission that the project owner sets. It cannot run code on the server. A host-side switch that turns agent access off is planned ([#118](https://github.com/LibrePaper/librepaper/issues/118)).
- **Code runs only on your own computer, and only when you ask.** Notebooks such as [Quarto](https://librepaper.org/notebooks/quarto.html) and [Calepin](https://librepaper.org/notebooks/calepin.html) run R or Python code, for example to draw a figure. That code runs only in the [desktop app](https://librepaper.org/desktop-app.html), never in a web browser, and only when you press **Run code**. An agent can start a run only if you allow it for that project. A host-side switch that turns Run code off is planned ([#119](https://github.com/LibrePaper/librepaper/issues/119)).

## What it does not protect

- **The operator can read everything.** Nothing is end-to-end encrypted. Whoever runs the server can read every document, comment and account, and so can anyone who breaks into it. Do not put confidential material on a server you do not control. The [privacy page](https://librepaper.org/privacy.html) says what librepaper.org stores.
- **Run code for computational notebooks is not sandboxed.** The code runs as you. It can read your files and reach the internet. Every editor of a document can change its code, and a compromised server could start a run. Press **Run code** only on documents whose editors you trust, and use the desktop app only with a server you trust.

## Hosting a server

The recommended deployment is very simple. The LibrePaper server is a single binary, and it needs a single PostgreSQL database. Run both [with Docker](https://librepaper.org/host/simple.html), with restricted filesystem access, on a machine that cannot reach your local network. Keep access to that machine strict, keep a backup that the server cannot delete, and upgrade promptly. [Security recommendations](https://librepaper.org/host/security.html) has the steps.

These habits make a takeover less likely and less costly. But, obviously, they do not completely eliminate the risks.
