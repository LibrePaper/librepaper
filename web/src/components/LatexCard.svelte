<script>
  // The card: what a person sees where the preview would be, the first time
  // they open a LaTeX document in this browser.
  //
  // Komodoc carries no TeX. The compiler is a set of static files a browser
  // fetches and runs, and this is the one place anyone is asked to fetch it.
  // Nothing is fetched until Choose is pressed -- not the module, not the
  // manifest's files, nothing -- and the editor beside this pane is usable
  // the whole time.
  //
  // Two rules about what is written here.
  //
  // The numbers are measured and are not this file's. They come from the
  // manifest, which `latex/mirror.mjs` writes from real file sizes and from
  // `examples/latex/MEASUREMENTS.md`. What this file decides is only that
  // *both* numbers are said. Every distribution defers most of its weight --
  // the format file and the packages arrive from inside the first compile --
  // so "1.9 MB" on its own would be off by an order of magnitude on the wait
  // a person actually sits through. The sentence therefore always has three
  // parts: to start, inside the first compile, and each document after.
  //
  // No distribution is named in this file. Which ones appear is the
  // manifest's `shown` flag, and so is which engine a deployment offers; a
  // self-hoster who fixes a bundle turns one on by rebuilding their mirror.
  import { onMount } from "svelte";
  import * as latex from "../lib/latex.js";

  let { onchosen, onerror } = $props();

  let distributions = $state([]);
  let preselected = $state(latex.chosen());
  let fetching = $state("");
  let progress = $state({ done: 0, total: 0 });
  let failure = $state("");

  onMount(async () => {
    try {
      distributions = await latex.available();
    } catch (error) {
      // A deployment whose mirror is unreachable has an empty card, and
      // saying so here beats an empty pane with no explanation in it.
      failure = error?.message || "the LaTeX mirror could not be reached";
    }
  });

  /// Megabytes, to one decimal below ten and none above: the difference
  /// between 17 MB and 18 MB is a fact, the difference between 17.38 and
  /// 17.41 is noise dressed as precision.
  function size(bytes) {
    const mb = (bytes || 0) / (1024 * 1024);
    if (mb < 0.1) return "nothing";
    if (mb < 1) return "well under 1 MB";
    return mb < 10 ? `${mb.toFixed(1)} MB` : `${Math.round(mb)} MB`;
  }

  const share = $derived(progress.total ? Math.round((progress.done / progress.total) * 100) : 0);

  async function choose(name) {
    if (fetching) return;
    fetching = name;
    failure = "";
    progress = { done: 0, total: 0 };
    try {
      await latex.choose(name, (at) => (progress = at));
      onchosen?.(name);
    } catch (error) {
      // The choice did not take, so the card stays and the same distribution
      // may be chosen again. `latex.js` has already forgotten it.
      failure = error?.message || "that distribution could not be loaded";
      onerror?.(failure);
    } finally {
      fetching = "";
    }
  }
</script>

<div class="latexcard">
  <h2 class="h4">Compile this document in your browser</h2>
  <p class="text-surface-700-300 text-sm">
    Komodoc ships no TeX. To see this paper as pages, choose a TeX distribution
    and your browser will fetch it — once, and keep it. Nothing is downloaded
    until you choose, and you can keep editing while it arrives.
  </p>

  {#if failure}
    <p class="badge preset-tonal-error">{failure}</p>
  {/if}

  {#each distributions as one (one.name)}
    <article class="card preset-tonal-surface distribution">
      <header>
        <h3 class="h5">{one.label}</h3>
        {#if one.name === preselected}
          <small class="badge preset-tonal-primary">chosen before</small>
        {/if}
      </header>
      <dl>
        <dt>Engines</dt>
        <dd>{one.engines.join(", ")}</dd>
        <dt>Licence</dt>
        <dd>{one.licence}</dd>
        <dt>Download</dt>
        <!-- The honest number, in three parts. See the note at the top of
             this file for why the first one is never printed alone. -->
        <dd>
          about {size(one.upfront)} to start, and about {size(one.first)} more inside
          the first compile — the LaTeX format and the packages the document asks
          for. Each further document costs {size(one.next)}.
        </dd>
        <dt>Trade</dt>
        <dd>{one.trade}</dd>
      </dl>
      {#if fetching === one.name}
        <div class="progress">
          <div class="bar" style:width="{share}%"></div>
        </div>
        <small class="text-surface-600-400">fetching… {share}%</small>
      {:else}
        <button type="button" class="btn preset-filled-primary-500" disabled={Boolean(fetching)}
                onclick={() => choose(one.name)}>
          Choose {one.label}
        </button>
      {/if}
    </article>
  {/each}

  {#if distributions.length}
    <p class="text-surface-600-400 text-xs">
      Choosing means this browser downloads that distribution once and keeps it,
      for this document and every other. It is asked for again only if your
      browser clears its storage. The choice is this browser's alone — it is not
      stored on the server and nobody else is affected by it. You can change it
      later from the toolbar.
    </p>
    <p class="text-surface-600-400 text-xs">
      A document is one directory here: its chapters, its <code>.bib</code> and its
      figures are compiled along with it, and nothing outside it is fetched.
    </p>
  {:else if !failure}
    <p class="text-surface-600-400 text-sm">Looking for the distributions this deployment serves…</p>
  {/if}
</div>

<style>
  .latexcard {
    display: flex;
    flex-direction: column;
    gap: var(--spacing);
    overflow-y: auto;
    padding: calc(var(--spacing) * 4);
    max-width: 44rem;
  }

  .distribution {
    display: flex;
    flex-direction: column;
    gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * 3);
  }

  .distribution header {
    display: flex;
    align-items: center;
    gap: var(--spacing);
  }

  .distribution dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--spacing) calc(var(--spacing) * 3);
    font-size: 0.875rem;
  }

  .distribution dt {
    font-weight: 600;
    white-space: nowrap;
  }

  .distribution button {
    align-self: flex-start;
  }

  /* A real bar over a real number: the up-front files are the ones this page
     fetches and the ones whose sizes the manifest knows. */
  .progress {
    height: calc(var(--spacing) * 2);
    border-radius: var(--radius-container);
    background: var(--color-surface-300);
    overflow: hidden;
  }

  .progress .bar {
    height: 100%;
    background: var(--color-primary-500);
    transition: width 120ms linear;
  }
</style>
