<script>
  import Modal from "./Modal.svelte";
  import { m } from "../lib/paraglide/messages.js";
  import { APPLE, caps, offeredWithUnavailable } from "../lib/commands.js";

  // Every command this workspace can currently run, by name. The same list the
  // keyboard and the menus read, filtered to what is available and asked for
  // in words rather than remembered as a chord.
  //
  // It runs the command, it does not reimplement it: what a line does here is
  // what the menu item does and what the shortcut does, because all three call
  // the one callback the reader passes in.
  //
  // A command the workspace cannot run here still matches by name when asked
  // for, so it is listed after the ones that can run, greyed, with the reason
  // the registry gives. It is not selectable: the highlight moves past it and
  // Enter does nothing on it.
  //
  // A dialog, not a floating box, so it inherits the one answer this
  // application has for what a dialog is: focus moves in, is trapped, Escape
  // closes, and the focus goes back where it came from. Nothing here listens
  // to the window, so a closed palette hears nothing at all.
  let { open = $bindable(false), context = {}, apple = APPLE, onrun } = $props();

  let query = $state("");
  let at = $state(0);

  const entries = $derived(offeredWithUnavailable(context, query, { apple }));
  const selectable = $derived(entries.filter((entry) => !entry.unavailable));
  // The highlight can outrun the list while the query is being typed: keep it
  // inside what is actually on the screen rather than pointing past the end.
  const current = $derived(Math.min(at, Math.max(selectable.length - 1, 0)));
  const chosen = $derived(selectable[current]);

  // A palette opens empty every time. The last thing somebody ran is not what
  // they came back for, and a prefilled field is a field to clear first.
  $effect(() => {
    if (open) { query = ""; at = 0; }
  });

  function run(entry) {
    if (!entry || entry.unavailable) return;
    open = false;
    // After the dialog, so the command acts on a page that has its focus back:
    // the panel commands move the focus themselves, and a dialog still closing
    // would take it straight back.
    queueMicrotask(() => onrun?.(entry.id));
  }

  function walk(event) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!selectable.length) return;
      const step = event.key === "ArrowDown" ? 1 : -1;
      at = (current + step + selectable.length) % selectable.length;
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      run(chosen);
    }
  }
</script>

<Modal bind:open title={m.commandpalette_title()} wide>
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
      placeholder={m.commandpalette_placeholder()}
      aria-label={m.common_command()}
      aria-expanded="true"
      aria-controls="palette-list"
      aria-activedescendant={chosen ? `palette-option-${chosen.id}` : undefined}
      oninput={() => (at = 0)}
      onkeydown={walk}
    />
    {#if entries.length}
      <ul class="results" id="palette-list" role="listbox" aria-label={m.commandpalette_list_label()}>
        {#each entries as entry (entry.id)}
          <!-- The line is the option, with nothing focusable inside it: the
               field keeps the focus while the palette is open and says which
               line is current through aria-activedescendant, which is what a
               combobox is. A button in here would be a control the keyboard
               is told about twice and can reach through neither. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li
            id="palette-option-{entry.id}"
            class="result"
            class:current={entry === chosen}
            class:unavailable={entry.unavailable}
            role="option"
            aria-selected={entry === chosen}
            aria-disabled={entry.unavailable || undefined}
            onmousemove={() => { if (!entry.unavailable) at = selectable.indexOf(entry); }}
            onclick={() => run(entry)}
          >
            <span class="result-name-block">
              <span class="result-name">{entry.label}</span>
              {#if entry.unavailable && entry.note}<span class="result-note">{entry.note}</span>{/if}
            </span>
            <span class="result-category">{entry.category}</span>
            {#if entry.keys.length}
              <span class="chord">{#each caps(entry.keys[0], apple) as cap}<kbd>{cap}</kbd>{/each}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="empty" role="status">{m.commandpalette_empty()}</p>
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
  .result.current { background: var(--color-row-selected); }
  .result.unavailable { color: var(--color-text-secondary); cursor: default; }
  .result.unavailable .result-name { color: var(--color-text-secondary); }
  .result-name-block { display: grid; min-width: 0; }
  .result-name { font-size: var(--text-sm); color: var(--color-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .result-note { font-size: var(--text-xs); color: var(--color-text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .result-category { font-size: var(--text-xs); color: var(--color-text-secondary); }
  .chord { display: inline-flex; gap: 2px; }
  .empty { font-size: var(--text-sm); color: var(--color-text-secondary); padding: calc(var(--spacing) * 2); }
  kbd {
    display: inline-block;
    min-width: 1.6em;
    padding: 1px calc(var(--spacing) * 1.25);
    border: 1px solid var(--color-border);
    border-bottom-width: 2px;
    border-radius: var(--radius-base);
    background: var(--color-raised);
    color: var(--color-text);
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
