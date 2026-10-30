<script>
  // How the source pane behaves for the person at this browser.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import ShortcutTable from "../ShortcutTable.svelte";
  import { FONTS, installed, MIN_SIZE, MAX_SIZE, font, setFont, onFont, size, setSize, onSize, lineHeight, setLineHeight, onLineHeight } from "../../lib/editor-type.js";

  let {
    keys = "default",
    onkeys,
    completion = true,
    oncompletion,
    brackets = true,
    onbrackets,
    proofing = "browser",
    onproofing,
    dialect = "american",
    ondialect,
    check = true,
    oncheck,
    visual = false,
    onvisual,
    commands = {},
  } = $props();

  // Follows the font, size and line height, which a pinch or another tab may change.
  let currentFont = $state(font());
  onMount(() => onFont((now) => { currentFont = now; }));

  let currentSize = $state(size());
  onMount(() => onSize((now) => { currentSize = now; }));

  let currentLineHeight = $state(lineHeight());
  onMount(() => onLineHeight((now) => { currentLineHeight = now; }));

  const PROOFING_DESCRIPTIONS = {
    off: "Nothing is underlined.",
    browser: "Your browser underlines misspelled words in prose, skipping commands, code, math and citation keys. Its language and dictionary decide what counts as a word.",
    harper: "Harper checks English spelling and grammar in prose. It downloads 7 MB the first time and uses about 300 MB of memory while a document is open. Documents that declare another language get the browser's spellcheck instead.",
  };
  const proofingDescription = $derived(PROOFING_DESCRIPTIONS[proofing] ?? PROOFING_DESCRIPTIONS.browser);
</script>

<SettingRow id="appearance-font" title="Font"
            description="Six fonts are included and download when you pick one. Another font has to be installed on this computer.">
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
      {#if currentFont.custom && !installed(currentFont.custom)}
        <span class="font-missing">Not found on this computer.</span>
      {/if}
    {/if}
  </span>
</SettingRow>

<SettingRow id="appearance-font-size" title="Font size"
            description="Pinch or Ctrl and scroll over the editor to change it there too.">
  <span class="setting-inline">
    <input type="number" class="input input-sm setting-input setting-number" min={MIN_SIZE} max={MAX_SIZE} step="1"
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

<SettingRow id="appearance-line-height" title="Line height">
  <select class="select setting-select" aria-label="Line height" value={currentLineHeight}
          onchange={(event) => setLineHeight(event.currentTarget.value)}>
    <option value="compact">Compact</option>
    <option value="normal">Normal</option>
    <option value="wide">Wide</option>
  </select>
</SettingRow>

<SettingRow id="editor-keys" title="Key bindings">
  <select class="select setting-select" aria-label="Editor keys" value={keys}
          onchange={(event) => onkeys?.(event.currentTarget.value)}>
    <option value="default">Standard</option>
    <option value="vim">Vim</option>
    <option value="emacs">Emacs</option>
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

<SettingRow id="editor-proofing" title="Grammar and spelling" description={proofingDescription}>
  <select class="select setting-select" aria-label="Grammar and spelling" value={proofing}
          onchange={(event) => onproofing?.(event.currentTarget.value)}>
    <option value="off">Off</option>
    <option value="browser">Browser</option>
    <option value="harper">Harper</option>
  </select>
</SettingRow>

{#if proofing === "harper"}
  <SettingRow id="editor-dialect" title="English dialect">
    <select class="select setting-select" aria-label="English dialect" value={dialect}
            onchange={(event) => ondialect?.(event.currentTarget.value)}>
      <option value="american">American</option>
      <option value="british">British</option>
      <option value="canadian">Canadian</option>
      <option value="australian">Australian</option>
      <option value="indian">Indian</option>
    </select>
  </SettingRow>
{/if}

<SettingRow id="editor-check" title="Code check"
            description="Underline errors and warnings from the last render in the editor. The list of messages stays available.">
  <button type="button" role="switch" class="switch" aria-checked={check} aria-label="Code check"
          data-state={check ? "checked" : "unchecked"} onclick={() => oncheck?.(!check)}>
    <span class="switch-thumb" data-state={check ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<SettingRow id="editor-visual" title="Visual mode"
            description="Open sources with headings, figures and maths drawn in place. The switch in the top bar changes it for one session.">
  <button type="button" role="switch" class="switch" aria-checked={visual} aria-label="Visual mode"
          data-state={visual ? "checked" : "unchecked"} onclick={() => onvisual?.(!visual)}>
    <span class="switch-thumb" data-state={visual ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<!-- The table is shown here and only here: `?` and the keyboard button both
     lead to this page. It comes last so it never pushes the switches above
     out of view. Stacked, because a table is not a control that fits beside
     a sentence. -->
<SettingRow id="editor-shortcuts" stacked title="Keyboard shortcuts"
            description="Press ? anywhere outside the editor to come back here.">
  <ShortcutTable context={commands} />
</SettingRow>

<style>
  /* A font name goes under the list when both do not fit beside the words. */
  .setting-inline {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: calc(var(--spacing) * 2);
  }
  .font-missing {
    flex-basis: 100%;
    text-align: right;
    color: var(--panel-muted);
    font-size: var(--panel-meta-size);
  }
  /* A size is two digits, so its field is narrow. */
  .setting-number {
    width: 5rem;
  }
</style>
