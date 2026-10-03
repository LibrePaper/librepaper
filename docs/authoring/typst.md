---
title: "Typst"
---

## Typst packages and fonts

- Packages imported from [Typst Universe](https://typst.app/universe); the compiler reports what it needs, the host fetches it, and the compile runs again
- Published versions never change, so the browser caches each forever; the command line keeps them in `~/.cache/typst/packages`
- Font families from the deployment's font library (configured by the operator; see [Fonts](../host.html#fonts)); font files beside the document used as `--font-path`
- Missing fonts are set to Typst's defaults and warned about
