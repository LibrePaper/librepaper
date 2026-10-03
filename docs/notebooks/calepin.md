---
title: "Calepin"
---

[Calepin](https://vincentarelbundock.github.io/calepin/) brings executable code chunks into native [Typst](https://typst.app) documents, so reports, papers and slides can compute tables, numbers and figures during rendering.

## Local app

The browser cannot run code; the LibrePaper local app runs Calepin on your computer with the tools installed there. Install from the [install page](../install.html), then start it:

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # check it is running and found Calepin
```

## Preview and output

Typst preview (in the browser, no code runs) is the default and always available. Choosing Calepin preview runs `calepin watch` on your computer, which re-renders on every change. Output is PDF or HTML:

- PDF shows in the viewer where comments work.
- HTML is a self-contained page.

Output is written beside the main file, same name, `.pdf` or `.html` extension.

Nothing rendered is ever uploaded. The server holds only the Typst source and its declared shared resources.

## Project folder

Calepin renders against a project folder on your disk bound to the document; see [the companion](../cli.html#the-companion). Choosing a folder does not upload its contents; the website receives an opaque binding identifier, not the folder's path. One preview per folder at a time.

If `librepaper status` does not find Calepin, install it or add it to PATH; a custom executable and arguments can be set in the settings page under the Calepin integration (it asks for confirmation in a native dialog).

## Trust

- Calepin runs with your user account: your files, installed packages and the network. LibrePaper does not sandbox it.
- Pairing asks once per site in a native dialog. Pairing alone runs nothing.
- Each document starts Calepin separately, after a warning that its code runs on your computer.
- Anyone with editor access can change that code, so start Calepin only on documents whose editors you trust.
