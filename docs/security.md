---
title: "Security"
---

This is a weird world. A single developer can now build a massive code base for a complex product in a matter of weeks or months. The norms of what counts as "good" or "careful" development are shifting, and the open source community has not converged on a consensus approach.

For LibrePaper, I try to follow best practices: I run several rounds of automated and supervised review, with multiple frontier agents. But no single developer can read or assess every line of a code base this large, this recent, and moving this fast. That raises important security concerns. Unfortunately, I do not have the budget for an independent external audit.

My primary audience for LibrePaper is:

- People who use the free service at librepaper.org.
- People who run LibrePaper on a spare laptop, a Raspberry Pi or a $6-a-month virtual private server.

LibrePaper is designed to serve hundreds of people on a machine with very limited resources, where the blast radius is small. I understand that institutions, such as the IT staff of a university, may be more reluctant to deploy it. I would be too! Still, some features of the architecture, and some habits on the host's side, limit the blast radius.

## What the design limits

- **Documents are never compiled on the server.** LaTeX, Typst, Quarto and friends never run there, and the server stores no compilers. Typesetting runs as WebAssembly inside the browser's sandbox, which gives you some protection from a hostile document.
- **The server runs no AI model.** An agent reaches LibrePaper only through an API, with a token or a connection that the project owner creates. Each one covers one project, carries one role (read, comment, suggest or edit) and can be revoked at any time. An agent cannot run code on the server. A host can turn connectors off with [`agent_connectors = false`](host/advanced.html#anonymous-access). A switch that turns all agent access off is planned ([#118](https://github.com/LibrePaper/librepaper/issues/118)).
- **Code runs only on your own computer, and only when you ask.** Notebooks such as [Quarto](notebooks/quarto.html) and [Calepin](notebooks/calepin.html) run R or Python code, for example to draw a figure. That code runs only in the [desktop app](desktop-app.html), never in a web browser, and only when you press **Run code**. An agent can start a run only if you turn on **Let agents run code** for that project. A host-side switch that turns Run code off is planned ([#119](https://github.com/LibrePaper/librepaper/issues/119)).
- **Published documents are kept away from your sign-in.** An HTML document can carry its own scripts. LibrePaper shows it in a sandboxed frame served from a second host, so those scripts cannot act as you in the app.
- **LibrePaper has no passwords.** People sign in with GitHub, Google, Microsoft Entra ID, ORCID or another OpenID Connect provider, so a breach cannot leak a password.
- **Sharing is closed by default.** A link requires sign-in unless both its owner and the host allow anonymous access. Someone who is not signed in can never edit or suggest.

## What it does not protect

- **The operator can read everything.** Nothing is end-to-end encrypted. Whoever runs the server can read every document, comment and account, and so can anyone who breaks into it. Do not put confidential material on a server you do not control. The [privacy page](privacy.html) says what librepaper.org stores.
- **The desktop app trusts its server.** A compromised server could start a run on the computer of anyone who uses the desktop app with it. Use the desktop app only with a server you trust.
- **Run code is not sandboxed.** The code runs as you. It can read your files and reach the internet. Every editor of a document can change its code, so press **Run code** only on documents whose editors you trust.
- **An agent reads untrusted text.** A shared document holds text from everyone who can suggest or edit, and an agent that reads it may follow instructions hidden there. Give an [agent](agents.html) the lowest role that does the job.
- **Data can be lost.** LibrePaper is experimental. Keep your own copy of work that matters, with [local backups](backups.html) or [Git](git.html).

## Hosting a server

The recommended deployment is very simple. The LibrePaper server is a single binary, and it needs a single PostgreSQL database. Run both with Docker, as in [Simple deployment](host/simple.html), and keep access to the machine strict.

- **Isolate the machine.** Use a VPS, or a virtual machine that cannot reach your local network. Keep it away from data that LibrePaper should never see.
- **Lock the door.** Allow SSH by key only. At your provider's firewall, open only ports 80 and 443 to the world, and keep SSH and the admin console to yourself. [Security recommendations](host/security.html) has the steps.
- **Choose who gets in.** In `librepaper.toml`, `publishers` and `commenters` list the accounts that may publish and comment. Name people or a domain, not `"*"`. See [Sign-in](host/simple.html#sign-in).
- **Turn off what you do not use.** Leave [anonymous access](host/advanced.html#anonymous-access) off, and turn agent connectors off if nobody needs them.
- **Keep a backup the server cannot delete.** Practice a restore before you need one. See [Backups](host/advanced.html#backups).
- **Upgrade promptly.** `./manage upgrade` [checks the release's signed attestation](host/security.html#upgrades) before it runs anything from it.

These habits make a takeover less likely and less costly. They do not make a deployment secure.

## Report a problem

Email [admin@librepaper.org](mailto:admin@librepaper.org). Do not open a public issue. My own review of the architecture is tracked in [#17](https://github.com/LibrePaper/librepaper/issues/17).
