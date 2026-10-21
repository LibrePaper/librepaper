<script>
  // How the source pane behaves for the person at this browser.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import ShortcutTable from "../ShortcutTable.svelte";
  import { FONTS, MIN_SIZE, MAX_SIZE, font, setFont, onFont, size, setSize, onSize, lineHeight, setLineHeight, onLineHeight } from "../../lib/editor-type.js";

  let {
    keys = "default",
    onkeys,
    completion = true,
    oncompletion,
    brackets = true,
    onbrackets,
    check = true,
    oncheck,
    commands = {},
  } = $props();

  // Follows the font, size and line height, which a pinch or another tab may change.
  let currentFont = $state(font());
  onMount(() => onFont((now) => { currentFont = now; }));

  let currentSize = $state(size());
  onMount(() => onSize((now) => { currentSize = now; }));

  let currentLineHeight = $state(lineHeight());
  onMount(() => onLineHeight((now) => { currentLineHeight = now; }));
</script>

<SettingRow id="editor-keys" title="Key bindings">
  <select class="select setting-select" aria-label="Editor keys" value={keys}
          onchange={(event) => onkeys?.(event.currentTarget.value)}>
    <option value="default">Standard</option>
    <option value="vim">Vim</option>
    <option value="emacs">Emacs</option>
  </select>
</SettingRow>

<SettingRow id="editor-font" title="Font"
            description="Fonts installed on this computer. LibrePaper does not download fonts.">
  <span class="setting-inline">
    <select class="select setting-select" aria-label="Font" value={currentFont.id}
            onchange={(event) => setFont(event.currentTarget.value, currentFont.custom)}>
      {#each FONTS as entry (entry.id)}
        <option value={entry.id}>{entry.name}</option>
      {/each}
    </select>
    {#if currentFont.id === "custom"}
      <input type="text" class="input input-sm setting-input" aria-label="Font name" placeholder="Font name"
             value={currentFont.custom}
             onchange={(event) => setFont("custom", event.currentTarget.value)} />
    {/if}
  </span>
</SettingRow>

<SettingRow id="editor-font-size" title="Font size"
            description="Pinch or Ctrl and scroll over the source to change it there too.">
  <span class="setting-inline">
    <input type="number" class="input input-sm setting-input" min={MIN_SIZE} max={MAX_SIZE} step="1"
           aria-label="Font size in pixels" value={currentSize}
           onchange={(event) => {
             const field = event.currentTarget;
             const next = field.value.trim() === "" ? NaN : Number(field.value);
             if (Number.isFinite(next)) setSize(next);
             field.value = String(size());
           }} />
    <span>px</span>
  </span>
</SettingRow>

<SettingRow id="editor-line-height" title="Line height">
  <select class="select setting-select" aria-label="Line height" value={currentLineHeight}
          onchange={(event) => setLineHeight(event.currentTarget.value)}>
    <option value="compact">Compact</option>
    <option value="normal">Normal</option>
    <option value="wide">Wide</option>
  </select>
</SettingRow>

<SettingRow id="editor-completion" title="Autocomplete"
            description="Suggest commands, labels and citations while you type.">
  <button type="button" role="switch" class="switch" aria-checked={completion} aria-label="Autocomplete"
          data-state={completion ? "checked" : "unchecked"} onclick={() => oncompletion?.(!completion)}>
    <span class="switch-thumb" data-state={completion ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<SettingRow id="editor-brackets" title="Close brackets"
            description="Typing an opening bracket adds the closing one.">
  <button type="button" role="switch" class="switch" aria-checked={brackets} aria-label="Close brackets"
          data-state={brackets ? "checked" : "unchecked"} onclick={() => onbrackets?.(!brackets)}>
    <span class="switch-thumb" data-state={brackets ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<SettingRow id="editor-check" title="Code check"
            description="Underline errors and warnings from the last render in the source. The list of messages stays available.">
  <button type="button" role="switch" class="switch" aria-checked={check} aria-label="Code check"
          data-state={check ? "checked" : "unchecked"} onclick={() => oncheck?.(!check)}>
    <span class="switch-thumb" data-state={check ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<!-- The table is shown here and only here: `?` and the keyboard button both
     lead to this page. It sits under the choice that changes half of it: the
     row above decides who owns the editing keys, and reading what they are is
     the next thing anybody does after changing it. Stacked, because a table is
     not a control that fits beside a sentence. -->
<SettingRow id="editor-shortcuts" stacked title="Keyboard shortcuts"
            description="Press ? anywhere outside the source to come back here.">
  <ShortcutTable context={commands} />
</SettingRow>

<style>
  .setting-inline {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 2);
  }
</style>
