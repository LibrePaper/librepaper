<script>
  // What a page that introduces itself opens with: the logo, mark and word in
  // one file, and the one line that says what LibrePaper is for. The logo is
  // the heading -- its alt text is the name -- so the page has an h1 without
  // setting the name twice. The heading takes the full row: the logo is sized
  // as a share of its container, and a heading shrink-wrapped by the column
  // would be sized by the logo in turn, a circle Firefox resolves to nothing.
  import Logo from "./Logo.svelte";
</script>

<div class="hero mb-8 flex flex-col items-center gap-3">
  <h1 class="w-full"><Logo size="hero" /></h1>
  <p class="text-surface-600-400">Annotate, comment, collaborate</p>
</div>
