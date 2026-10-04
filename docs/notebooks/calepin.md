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

Calepin runs locally and its generated output is not uploaded automatically. The hosted project does hold its shared Typst source and resources. Authorized readers and commenters receive the source projection and shared assets needed for browser rendering, so keep private inputs outside the shared project.

## Project folder

Calepin renders against a project folder on your disk bound to the document; see [the companion](../cli.html#the-companion). Choosing a folder does not upload its contents; the website receives an opaque binding identifier, not the folder's path. One preview per folder at a time.

If `librepaper status` does not find Calepin, install it or add it to PATH; a custom executable and arguments can be set in Settings under Integrations → Calepin.

## Trust

- Calepin runs with your user account: your files, installed packages and the network. LibrePaper does not sandbox it.
- Pairing asks once per site in Companion Settings. Check the site and requested action before allowing it. Pairing alone runs nothing.
- Every document starts on the browser preview. Calepin runs only after you choose View > Execute code locally and accept the warning, one document at a time.
- Anyone with editor access can change that code at any time, so allow it only on documents whose owner and editors you trust.
- The permission is remembered for that document in this browser until you turn it off or sign out.
