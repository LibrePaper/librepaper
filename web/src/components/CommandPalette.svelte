<script>
  import Modal from "./Modal.svelte";
  import { APPLE, caps, filter, offered } from "../lib/commands.js";

  // Every command this workspace can currently run, by name. The same list the
  // keyboard and the menus read, filtered to what is available and asked for
  // in words rather than remembered as a chord.
  //
  // It runs the command, it does not reimplement it: what a line does here is
  // what the menu item does and what the shortcut does, because all three call
  // the one callback the reader passes in.
  //
  // A dialog, not a floating box, so it inherits the one answer this
  // application has for what a dialog is: focus moves in, is trapped, Escape
  // closes, and the focus goes back where it came from. Nothing here listens
  // to the window, so a closed palette hears nothing at all.
  let { open = $bindable(false), context = {}, apple = APPLE, onrun } = $props();

  let query = $state("");
  let at = $state(0);

  const entries = $derived(filter(offered(context, { apple }), query));
  // The highlight can outrun the list while the query is being typed: keep it
  // inside what is actually on the screen rather than pointing past the end.
  const current = $derived(Math.min(at, Math.max(entries.length - 1, 0)));

  // A palette opens empty every time. The last thing somebody ran is not what
  // they came back for, and a prefilled field is a field to clear first.
  $effect(() => {
    if (open) { query = ""; at = 0; }
  });

  function run(entry) {
    if (!entry) return;
    open = false;
    // After the dialog, so the command acts on a page that has its focus back:
    // the panel commands move the focus themselves, and a dialog still closing
    // would take it straight back.
    queueMicrotask(() => onrun?.(entry.id));
  }

  function walk(event) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!entries.length) return;
      const step = event.key === "ArrowDown" ? 1 : -1;
      at = (current + step + entries.length) % entries.length;
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      run(entries[current]);
    }
  }
</script>

<Modal bind:open title="Run a command" wide>
  <div class="palette">
    <!-- svelte-ignore a11y_autofocus -->
    <input
      bind:value={query}
      class="input"
      type="text"
      role="combobox"
      autofocus
      autocomplete="off"
      spellcheck="false"
      placeholder="Type a command"
      aria-label="Command"
      aria-expanded="true"
      aria-controls="palette-list"
      aria-activedescendant={entries.length ? `palette-option-${entries[current].id}` : undefined}
      oninput={() => (at = 0)}
      onkeydown={walk}
    />
    {#if entries.length}
      <ul class="results" id="palette-list" role="listbox" aria-label="Commands">
        {#each entries as entry, index (entry.id)}
          <!-- The line is the option, with nothing focusable inside it: the
               field keeps the focus while the palette is open and says which
               line is current through aria-activedescendant, which is what a
               combobox is. A button in here would be a control the keyboard
               is told about twice and can reach through neither. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li
            id="palette-option-{entry.id}"
            class="result"
            class:current={index === current}
            role="option"
            aria-selected={index === current}
            onmousemove={() => (at = index)}
            onclick={() => run(entry)}
          >
            <span class="result-name">{entry.label}</span>
            <span class="result-category">{entry.category}</span>
            {#if entry.keys.length}
              <span class="chord">{#each caps(entry.keys[0], apple) as cap}<kbd>{cap}</kbd>{/each}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="empty" role="status">No command matches that.</p>
    {/if}
  </div>
</Modal>

<style>
  .palette { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); min-height: 0; }
  .results { max-height: 22rem; overflow-y: auto; margin: 0; padding: 0; list-style: none; }
  .result {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    width: 100%;
    padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 2);
    border-radius: var(--radius-base);
    text-align: left;
    cursor: pointer;
  }
  .result.current { background: var(--color-primary-100-900); }
  .result-name { font-size: var(--text-sm); color: var(--color-surface-900-100); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .result-category { font-size: var(--text-xs); color: var(--color-surface-600-400); }
  .chord { display: inline-flex; gap: 2px; }
  .empty { font-size: var(--text-sm); color: var(--color-surface-600-400); padding: calc(var(--spacing) * 2); }
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
  @media (max-width: 560px) {
    .result { grid-template-columns: 1fr auto; }
    .result-category { display: none; }
  }
</style>
