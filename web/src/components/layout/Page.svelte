<script>
  // A page: one column of a readable width, centred, with the bar above it.
  // Every screen that is not the reader is one of these, so the margins and
  // the measure are decided once.
  let { width = "measure", children } = $props();

  const WIDTHS = {
    measure: "max-w-3xl", // prose, and forms about one thing
    wide: "max-w-6xl", // a table, which wants the room
  };
</script>

<main id="main" tabindex="-1" class="mx-auto w-full {WIDTHS[width]} px-4 py-8">
  {@render children()}
</main>
