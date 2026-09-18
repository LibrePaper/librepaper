<script>
  // What a page that introduces itself opens with: the logo, mark and word in
  // one file, and the one line that says what LibrePaper is for. The logo is
  // the heading -- its alt text is the name -- so the page has an h1 without
  // setting the name twice. The heading takes the full row: the logo is sized
  // as a share of its container, and a heading shrink-wrapped by the column
  // would be sized by the logo in turn, a circle Firefox resolves to nothing.
  //
  // Under the line, the formats. They are the answer to the first question a
  // visitor has -- will it take what I already write in -- so they are said
  // before anything else on the page, and said quietly: small caps in the
  // muted grey, a row of names rather than a feature list. The separators are
  // markup rather than text so a screen reader reads five names, not four
  // middots.
  import Logo from "./Logo.svelte";

  // LaTeX is written as its own wordmark in the markup; these are the plain
  // names that follow it.
  const rest = ["Typst", "Markdown", "Computational notebooks"];
</script>

<div class="hero mb-8 flex flex-col items-center gap-3">
  <h1 class="w-full"><Logo size="hero" /></h1>
  <p class="text-surface-600-400">Annotate, comment, collaborate</p>
  <p class="formats text-surface-600-400">
    <span>HTML</span>
    <span aria-hidden="true" class="dot">·</span>
    <span class="latex">L<span class="latex-a">a</span>T<span class="latex-e">e</span>X</span>
    {#each rest as format}
      <span aria-hidden="true" class="dot">·</span>
      <span>{format}</span>
    {/each}
  </p>
</div>

<style>
  /* One row of names that wraps as a row of names: the separators wrap with
     the word they follow rather than starting a line. */
  .formats {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    align-items: baseline;
    gap: 0 calc(var(--spacing) * 2);
    margin: 0;
    font-size: var(--text-sm);
    font-variant-caps: all-small-caps;
    letter-spacing: .08em;
  }

  .dot { letter-spacing: 0; opacity: .6; }

  /* The wordmark, at the size of its neighbours: the two letters that move
     are the whole of the joke, so they move and nothing else does. Small caps
     already flattens the case, which is why the A and the E are written in
     lowercase -- the shapes come from the font's small capitals either way,
     and a browser without them still reads "LaTeX" rather than "LATEX". */
  .latex { letter-spacing: 0; }
  .latex-a { font-size: .9em; vertical-align: .22em; margin-left: -.32em; margin-right: -.12em; }
  .latex-e { vertical-align: -.22em; margin-left: -.1em; margin-right: -.06em; }
</style>
