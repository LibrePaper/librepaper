---
title: "Typst"
---

## Typst packages and fonts

A Typst document may import a package from
[Typst Universe](https://typst.app/universe) and name any font family. The
compiler, in the browser and in `publish` alike, never fetches anything
itself: it says what it went looking for and did not find, the host fetches
that, and the compile runs again. Packages come straight from the registry --
a published version never changes, so the browser caches each forever and the
command line keeps them where the `typst` binary keeps its own, under
`~/.cache/typst/packages`. A font file beside the document is used as
`--font-path` would use it.

Any other family comes from the deployment's own font library, which an
operator configures; see [Fonts](../host.html#fonts). The editor asks for a
family the compiler warned about, and publishing asks the same deployment for
the same files, so the preview uses the same faces. Where a deployment offers
no library, a document naming a family the compiler does not embed is set in
Typst's default faces and warned about, as it would be on a machine without
that font installed.

## Typst documents with Calepin

Ordinary Typst rendering — the browser's own compiler, described above — is
unchanged. A Typst document with code chunks can also offer **Calepin
preview** under Tools alongside **Typst preview** (the default): it runs
`calepin watch` on your own computer, through the local app, and shows the
resulting PDF in the usual PDF viewer, where comments work. This needs
Calepin installed on the machine running the local app; pairing works
exactly as it does for Quarto preview —
one **Allow** click, no binding required. If Calepin isn't installed, the
banner says so. Nothing rendered is ever uploaded.
