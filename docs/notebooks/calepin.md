---
title: "Calepin"
---

Calepin renders Typst documents with executable code chunks to PDF.

## Local app

The browser cannot run code in Typst documents; the LibrePaper local app does that on your computer with Calepin and the tools installed there. Install from the [install page](../install.html), then start it:

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # check it is running and found Calepin
```

> **Warning:** Previews run the document's code, filters and scripts on your computer, so only enable this on documents you trust. Anyone with editor access can change that code.

## Rendering and output

Calepin preview runs `calepin watch` on your computer and shows the resulting PDF in the viewer, where comments work. Typst preview remains the default preview mode and is always available in the browser.

Nothing rendered is ever uploaded.
