<script>
  import { APPLE, helpTable } from "../lib/commands.js";

  // What the keyboard does, drawn from the same list the keyboard is wired
  // from. Nothing here is written out: a row exists because a command exists,
  // its keys are that command's bindings, and a command that loses its binding
  // loses its row on the same edit. A table typed out by hand is a table that
  // describes the application as it was.
  //
  // Unavailable commands are shown greyed rather than hidden. The question
  // this table answers is "what can I press", and a key that vanishes because
  // the preview happens to be closed reads as a key that does not exist.
  //
  // It is a component rather than markup inside the dialog because it is
  // wanted in two places: behind `?`, where somebody is asking right now, and
  // in the settings beside the choice of editor keys, where somebody is
  // reading about the workspace rather than using it. One table, so the two
  // cannot come to say different things.
  let { context = {}, modalEditor = false, apple = APPLE } = $props();

  const groups = $derived(helpTable(context, { apple }));
</script>

<div class="shortcuts">
  {#each groups as group (group.name)}
    <section class="group" aria-labelledby="shortcuts-{group.name}">
      <h3 class="group-name" id="shortcuts-{group.name}">{group.name}</h3>
      <table class="keytable">
        <thead class="sr-only">
          <tr><th scope="col">Command</th><th scope="col">Shortcut</th><th scope="col">Where it works</th></tr>
        </thead>
        <tbody>
          {#each group.rows as row (row.id)}
            <tr class:unavailable={!row.available}>
              <th scope="row" class="command">{row.label}{#if !row.available}<span class="sr-only"> (not available here)</span>{/if}</th>
              <td class="keys">
                {#if row.keys.length}
                  {#each row.keys as binding, index}
                    {#if index}<span class="alias">or</span>{/if}
                    <span class="chord">
                      {#each binding as cap}<kbd>{cap}</kbd>{/each}
                    </span>
                  {/each}
                {:else}
                  <span class="nokeys">Command palette</span>
                {/if}
              </td>
              <td class="note">{row.note}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/each}
  <!-- The two honest disclaimers. The first is why ⌘T is not in the table
       above and never will be; the second is why the Editing rows are a
       description of CodeMirror rather than a promise this application
       keeps. -->
  <p class="footnote">
    Browser shortcuts stay with the browser. Editing keys belong to the source
    editor, so they work where the text does.
  </p>
  {#if modalEditor}
    <p class="footnote">
      Vim and Emacs mode shortcuts are provided by the selected editor mode. The
      workspace shortcuts above remain available where they do not conflict with
      that mode.
    </p>
  {/if}
</div>

<style>
  .shortcuts { display: flex; flex-direction: column; gap: calc(var(--spacing) * 4); }
  .group-name { font-size: var(--text-xs); font-weight: 600; text-transform: uppercase; letter-spacing: .06em; color: var(--color-surface-600-400); margin-bottom: var(--spacing); }
  .keytable { width: 100%; border-collapse: collapse; }
  .keytable tr { border-top: 1px solid var(--color-surface-200-800); }
  .keytable tr:first-child { border-top: 0; }
  .keytable th, .keytable td { padding: calc(var(--spacing) * 1.5) 0; text-align: left; vertical-align: baseline; font-weight: 400; }
  .command { font-size: var(--text-sm); color: var(--color-surface-900-100); width: 45%; }
  .keys { white-space: nowrap; width: 30%; }
  .note { font-size: var(--text-xs); color: var(--color-surface-600-400); text-align: right; }
  /* Greyed, not hidden, and said in words as well as in colour: a row a
     screen reader reaches has to say it is unavailable too. */
  .unavailable { opacity: .45; }
  .chord { display: inline-flex; gap: 2px; }
  .alias { font-size: var(--text-xs); color: var(--color-surface-500); padding-inline: calc(var(--spacing) * .75); }
  .nokeys { font-size: var(--text-xs); color: var(--color-surface-500); }
  kbd {
    display: inline-block;
    min-width: 1.6em;
    padding: 1px calc(var(--spacing) * 1.25);
    border: 1px solid var(--color-surface-300-700);
    border-bottom-width: 2px;
    border-radius: var(--radius-base);
    background: var(--color-surface-50-950);
    color: var(--color-surface-800-200);
    font-family: inherit;
    font-size: var(--text-xs);
    line-height: 1.5;
    text-align: center;
  }
  .footnote { font-size: var(--text-xs); color: var(--color-surface-600-400); }
  /* Narrow: the three columns become one, and each row a small card. The
     command, its keys and where it works still read as one thing, which is
     the only part of the layout that matters. */
  @media (max-width: 560px) {
    .keytable, .keytable tbody, .keytable tr, .keytable th, .keytable td { display: block; width: auto; }
    .keytable tr { padding-block: calc(var(--spacing) * 1.5); }
    .keytable th, .keytable td { padding: 0; }
    .command { font-weight: 500; }
    .keys { margin-top: calc(var(--spacing) * .75); white-space: normal; }
    .note { text-align: left; margin-top: calc(var(--spacing) * .5); }
  }
</style>
