---
title: "Calepin"
---

[Calepin](https://vincentarelbundock.github.io/calepin/) brings executable code chunks into native [Typst](https://typst.app) documents, so reports, papers and slides can compute tables, numbers and figures during rendering.

## Companion

The browser cannot run code. The LibrePaper companion runs Calepin on your computer with the tools installed there. Install [the companion](../local-app.html), then start it:

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # check it is running and found Calepin
```

## Preview and output

Typst preview runs in the browser, and no code runs. It is the default and always available. Choosing Calepin preview runs `calepin watch` on your computer, which re-renders on every change. Output is PDF or HTML:

- PDF shows in the viewer where comments work.
- HTML is a self-contained page.

Output is written beside the main file as `.pdf` or `.html` with the same name.

Calepin runs locally and its generated output is not uploaded automatically. The project's Typst source and resources are stored on the server. Anyone you share the project with can read them, so keep private data in your project folder.

## Project folder

Calepin can render with a folder on your computer. This is how you use private data without uploading it.

- Start the companion.
- Choose **File → Project folder…**.
- Check the main file name.
- Click **Choose folder…** and approve.

Choosing a folder does not upload its contents, and LibrePaper never learns its path. Each folder runs one preview at a time.

Before each render, LibrePaper writes the project's files into that folder. The folder always matches the editor.

- LibrePaper never touches other files there, such as data.
- LibrePaper never deletes anything.
- If you changed one of the project's files in that folder yourself, LibrePaper keeps your version as `<name>.local` before it writes. For example, `paper.typ.local`.

If `librepaper status` does not find Calepin, install it or add it to PATH. A custom executable and arguments can be set in Settings → Render → Typst and Calepin.

## Trust

- Calepin runs with your user account. It can use your files, installed packages and the network. LibrePaper does not sandbox it.
- Pairing asks once per site in Settings → Render → Typst and Calepin. Check the site and requested action before allowing it. Pairing alone runs nothing.
- Every document starts on the browser preview. Calepin runs only after you choose View > Execute code locally and accept the warning, one document at a time.
- Anyone with editor access can change that code at any time, so allow it only on documents whose owner and editors you trust.
- The permission is remembered for that document in this browser until you turn it off or sign out.
