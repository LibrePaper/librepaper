<script>
  // The mark alone on the bar, where there is no width to spare; the mark and
  // the word above a page that introduces itself. The mark is a page in two
  // puzzle pieces, the left in the site's pine and the right in its ink, and
  // the word beside it is the same two colours: "Libre" in pine, "Paper" in
  // ink. The word is set in Gotham HTF and outlined into paths in the file,
  // so no brand font is served and nobody sees it fall back. Both files are
  // exported from the sources at the root of the repository. Both carry a
  // width and height as well as a viewBox: an SVG with only a viewBox has no
  // intrinsic size in an <img>, and a browser sizing it from its container
  // alone falls back to 300px, or in Firefox to nothing at all.
  //
  // Two sizes: the one on the bar, and the one above a page that introduces
  // itself. A caller asks for a size rather than reaching in with a selector,
  // which is what lets these styles stay scoped to this file.
  let { size = "bar" } = $props();
</script>

{#if size === "hero"}
  <img class="logo hero" src="/assets/librepaper-logo.svg" alt="LibrePaper" />
{:else}
  <img class="logo bar" src="/assets/librepaper-icon.svg" alt="LibrePaper" />
{/if}

<style>
  .logo {
    display: block;
  }
  /* Sized by height, so the bar decides how much room it takes. */
  .bar {
    height: 1.4rem;
    width: auto;
    aspect-ratio: 1;
  }
  @media (max-width: 760px) {
    .bar {
      height: 1.05rem;
    }
  }
  /* Sized by width, because the word is nearly seven times as wide as it is
     tall: a phone gets the whole of it across the screen rather than a height
     that would run off the edge. */
  .hero {
    width: min(100%, 28rem);
    height: auto;
    aspect-ratio: 6221 / 908;
    margin-inline: auto;
  }
</style>
