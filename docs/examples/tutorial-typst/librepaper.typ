= Learn LibrePaper with Typst

#figure(image("librepaper-icon.png", width: 24%), caption: [The LibrePaper icon, loaded as a relative project asset.])

Welcome to LibrePaper. Edit this sentence in the browser and watch the preview update. Your Typst source, comments, highlights, and checkpoints travel with the document.

== Source and preview

This file is Typst. LibrePaper's pinned compiler runs in a browser WebAssembly worker and in the native command-line client. The server synchronizes the source and stores successful PDFs; it does not compile the document.

== A small scientific example

For a sample of size $n$, the standard error of a mean is

$ "SE" = s / sqrt(n) $

#include "sections/rendering.typ"

The icon above is a relative project asset. Change its width, then make a checkpoint so a collaborator can compare the edit with the previous version.

== What Typst adds

Typst combines markup, math, and scripting in one compact source file. A small function can keep repeated labels consistent:

#let estimate(value, unit: "") = [estimate: #value#unit]

#estimate(0.42)

Try changing the estimate and leave a comment on this paragraph. Typst Universe packages can be imported with `#import`; LibrePaper fetches requested packages through the host and recompiles with the immutable package version.

== Working with other clients

Choose Typst PDF preview for printed layout or experimental Typst HTML preview for flowing, semantic output. If a document contains executable Calepin chunks, the companion can offer a Calepin PDF preview. Use `librepaper sync` for a local editor and `librepaper publish` to publish from the terminal. Readers receive the stored PDF and do not need Typst installed.
