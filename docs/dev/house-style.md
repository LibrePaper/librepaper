<!-- The design notes for web/src/styles/document.css, the house stylesheet every LibrePaper HTML document is shown in. Moved from LibrePaper/wasm-helpers (docs/style.md) when the stylesheet moved into the app (kata 6y41). The class vocabulary the renderers emit stays in wasm-helpers: docs/classes.md. -->

# House style v2 — modern serif, intentional choices for every element

Direction: a contemporary academic web document. Serif for reading, a quiet
system sans for interface-like labels, lining numerals, neutral surfaces, no
small caps, no ornament. It should feel like a well-made journal article on
the web, not a facsimile of a printed book, and not like Bootstrap/Quarto.

## Tokens
- `--lp-serif`: "Charter", "Bitstream Charter", "Source Serif 4", "Source Serif Pro", "Sitka Text", "Noto Serif", Cambria, Georgia, serif  (drop "Iowan Old Style": old-style face)
- `--lp-sans` (new): system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif
- `--lp-mono`: keep.
- Colours: keep the neutral paper/ink and derived shades (rq34). Accent: keep #1f5fa8 light / #8ab4f0 dark. Add `--lp-mark` modern highlight: light `color-mix(in srgb, #ffd84d 45%, transparent)`, dark `color-mix(in srgb, #ffd84d 25%, transparent)`.
- Numerals: body `font-variant-numeric: lining-nums proportional-nums`; tables, equation numbers, section numbers, TOC numbers, footnote numbers: `lining-nums tabular-nums`. No oldstyle-nums anywhere.
- No `font-variant: small-caps` / `font-variant-caps` anywhere except `.smallcaps`. No `text-transform: uppercase` labels.

## Page
- body: serif 1.0625rem / 1.65, ink on paper, max-width 68ch, margin auto, padding 3rem 1.25rem 5rem (phone: 1.5rem 1rem 3rem). `text-wrap: pretty`, `hanging-punctuation: first`, `-webkit-font-smoothing: antialiased`.
- Paragraph spacing 1em; no indents.
- Selection colour: accent at 20%.

## Headings
- Serif, weight 650, `letter-spacing: -0.012em`, `text-wrap: balance`, line-height 1.2, ink colour. No rules/borders under any heading.
- Sizes (body headings; the document title is separate): h1 1.75rem (margin 2.6rem 0 .8rem), h2 1.35rem (2.2rem 0 .6rem), h3 1.125rem (1.8rem 0 .5rem), h4 1rem (1.5rem 0 .4rem), h5/h6 1rem regular italic.
- Section number (`.section-number[data-number]::before`): same font and size, weight 400, colour muted, tabular lining, margin-right .5em.
- Anchors (`a.anchor`): hidden.

## Title block (`header.title-block`)
- Left-aligned (not centred), no rules inside; a single hairline (`--lp-rule`) below the whole block, padding-bottom 1.5rem, margin-bottom 2rem.
- `.title`: serif 650, `font-size: clamp(1.8rem, 1.2rem + 2.4vw, 2.5rem)`, line-height 1.12, letter-spacing -0.02em, margin 0 0 .4rem.
- `.subtitle`: serif regular (not italic), 1.2rem, muted, margin 0 0 1rem.
- `.author`: serif 1rem ink; `.affiliation-ref` sup in sans .7em muted.
- `.affiliations`, `.date`: sans .85rem muted, margin .2rem 0.
- `.abstract`: margin 1.25rem 0 0; `.abstract-title`: sans .8rem weight 600 ink, letter-spacing .01em, margin 0 0 .3rem (sentence case "Abstract"); abstract text serif 1rem, ink.
- `.keywords`: sans .85rem muted; `.keywords-label` weight 600 ink.

## Table of contents (`nav.toc`)
- sans .9rem, margin 0 0 2.25rem; no top/bottom rules; `.toc-title`: sans .8rem 600 muted, margin 0 0 .5rem.
- Lists: no bullets, padding 0; nested lists padding-left 1.1rem; items .15rem apart; links ink (not accent), no underline, hover accent; numbers muted tabular with a fixed min-width (2.2em) so titles align.

## Text-level
- Links: accent; underline 1px, `text-underline-offset: .18em`, decoration colour accent 35%, hover full accent. Crossref links (`.crossref`, `.quarto-crossref`): no underline until hover.
- `strong` 650; `em` italic.
- Inline `code`: mono .875em, `--lp-code-bg`, radius 3px, padding .1em .3em, no border.
- `kbd`: mono .8em, padding .05em .4em, border 1px `--lp-rule`, border-bottom 2px, radius 4px, background paper.
- `mark` / `.mark`: background `--lp-mark`, padding 0 .1em, radius 2px, colour inherit.
- `.smallcaps`: small-caps (only here).
- `sub`/`sup`: .75em, line-height 0.
- Footnote refs: sans .7em, accent, no underline.

## Blocks
- Blockquote: not italic; border-left 3px `--lp-rule`, padding .1rem 0 .1rem 1.1rem, margin 1.5rem 0, colour ink at 85% (color-mix ink 85% into paper). Last paragraph that starts with an em dash is the attribution: sans .85rem muted (selector: `blockquote p:last-child` when it begins with "—" can't be detected in CSS — just style `blockquote footer, blockquote cite` and leave paragraphs as body).
- Lists: padding-left 1.4rem; markers muted (`::marker { color: var(--lp-muted) }`); `li + li` margin .25rem; nested lists margin .25rem 0. Task lists: `input[type=checkbox]` accent-color accent, margin-right .45em, vertical-align -0.1em; list-style none for task items (`li:has(> input[type=checkbox])`).
- Definition lists: `dt` weight 650 margin-top .9rem; `dd` margin .2rem 0 0 1.25rem.
- `hr`: border 0, border-top 1px `--lp-rule`, margin 2.5rem 0.

## Callouts
- Background `color-mix(in srgb, var(--k) 6%, var(--lp-paper))` (dark: 10%), border-left 3px solid `var(--k)`, radius 0 6px 6px 0, padding .8rem 1rem, margin 1.5rem 0.
- `.callout-title`: sans .85rem weight 600, colour `var(--k)`, margin 0 0 .25rem, sentence case — NO small caps, no uppercase.
- Body: serif, ink; first/last child margins 0.
- Collapsible (`details.callout`): summary with a small chevron (CSS-drawn, rotates when open), cursor pointer, no default marker.
- `.callout-simple`: no background tint; `.callout-minimal`: no background, title hidden.

## Figures and tables
- `figure` margin 1.75rem 0; images centred, max-width 100%, height auto.
- `figcaption`: sans .85rem / 1.45, muted, margin-top .6rem (tables: caption above, margin-bottom .5rem), text-align left. `.caption-label`: weight 600 ink, margin-right .3em (keep "Figure 1." wording).
- Figure grids and subfigures: unchanged, captions as above.
- Tables: font-size .95rem, lining tabular nums, border-collapse; booktabs: top rule 1.5px ink 85%, header bottom 1px ink 60%, bottom rule 1.5px ink 85%; `th` serif weight 650 ink, vertical-align bottom; `td,th` padding .45rem .75rem; first/last column padding-left/right 0 so the rules align with the text; no zebra; right-aligned cells nowrap.
- Equations (`.equation`): margin 1.4rem 0; `.equation-number` muted tabular lining.

## Theorems and proofs
- Margin 1.4rem 0. `.theorem-label`: serif 650 ink; `.theorem-name`: regular ink; body italic for thm/lem/cor/prp/cnj (convention), upright for def/exm/exr/sol/rem.
- `.proof-label`: italic; end of proof: `∎` in muted, placed at the end of the last line (float right).

## Code and output
- Keep rq34 exactly (subtle code panel, verbatim output, error/warning rules). Code filename and fold summary: sans .8rem muted (not mono).
- Tab sets (`.tabset`): `.tab-title` sans .8rem 600 muted, margin 1rem 0 .35rem, no small caps, no rule.

## Footnotes (`section.footnotes`)
- Margin-top 3rem, border-top 1px `--lp-rule`, padding-top 1rem, sans .85rem / 1.5, muted; `ol` padding-left 1.25rem; backref inline immediately after the text (no gap), accent, no underline.

## Margin content (`aside.margin`)
- sans .8rem / 1.45 muted; wide screens: floats into the right gutter at ≥1180px (as now); narrow: block with left rule.

## Columns, video, misc
- Columns: gap 2rem; stack below 640px. Video 16:9 as now.

## Dark mode
- Same rules; check: callout tints readable, table rules visible (ink 85% on dark is light), mark not glaring, code panel distinct but quiet.

## Adapters
- LaTeXML and Typst adapters: map onto the same choices (title left-aligned, sans labels, no small caps, captions sans).
- Quarto page adapter (`.lp-quarto`): same — `.callout-title-container` sans 600 sentence case, `#title-block-header` left-aligned, `#TOC` like nav.toc, `figcaption.quarto-float-caption` sans muted.

## Print
- Light palette (as now); body 11pt / 1.45; links ink without underline; no page-breaks inside figures/tables/callouts/theorems.

## Implementation notes (deviations from the spec)
- `--lp-tint` (6% light, 10% dark) carries the callout tint so no rule depends on a `.lp-dark` ancestor selector (the editor scopes the sheet).
- The dark palette sits under `@media not print`, so print needs no palette copy.
- `.ltx_font_smallcaps` is grouped with `.smallcaps`: authored small caps survive; nothing else uses small caps.
- Blockquote `footer`/`cite` carry the attribution; paragraphs stay body text.
- `.callout-simple` and `.callout-minimal` both drop the tint; only minimal hides the title.
