---
title: "Calepin"
---

[Calepin](https://vincentarelbundock.github.io/calepin/) brings executable code chunks into native [Typst](https://typst.app) documents, so reports, papers and slides can compute tables, numbers and figures during rendering.

## Desktop app

The browser cannot run code. The [LibrePaper desktop app](../desktop-app.html) runs Calepin on your computer with the tools installed there.

## Preview and output

Typst preview runs in the browser, and no code runs. It is the default and always available. Choosing Calepin preview runs `calepin watch` on your computer, which re-renders on every change. Output is PDF or HTML:

- PDF shows in the viewer where comments work.
- HTML is a self-contained page.

Output is written beside the file being previewed as `.pdf` or `.html` with the same name.

Calepin runs locally and its generated output is not uploaded automatically. The project's Typst source and resources are stored on the server. Anyone you share the project with can read them, so keep private data out of the project.

If the desktop app does not find Calepin, install it or add it to PATH. A custom executable and arguments can be set in Settings → Render → Typst and Calepin.

## Trust

- Calepin runs with your user account. It can use your files, installed packages and the network. LibrePaper does not sandbox it.
- Calepin runs only in the [desktop app](../desktop-app.html). The browser cannot run it.
- Every document opens with **Run code** off, on the browser preview. Turn on **Run code** at the top of the editor to run Calepin. It is off again the next time you open the document.
- Anyone with editor access can change that code at any time, so allow it only on documents whose owner and editors you trust.
