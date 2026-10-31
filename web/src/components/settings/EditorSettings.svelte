<script>
  // How the source pane behaves for the person at this browser.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import ShortcutTable from "../ShortcutTable.svelte";
  import { FONTS, installed, MIN_SIZE, MAX_SIZE, font, setFont, onFont, size, setSize, onSize, lineHeight, setLineHeight, onLineHeight } from "../../lib/editor-type.js";
  import { m } from "../../lib/paraglide/messages.js";

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
    off: m.editorsettings_proofing_description_off(),
    browser: m.editorsettings_proofing_description_browser(),
    harper: m.editorsettings_proofing_description_harper(),
  };
  const proofingDescription = $derived(PROOFING_DESCRIPTIONS[proofing] ?? PROOFING_DESCRIPTIONS.browser);
</script>

<SettingRow id="appearance-font" title={m.common_font()}
            description={m.editorsettings_font_description()}>
  <span class="setting-inline">
    <select class="select setting-select" aria-label={m.common_font()} value={currentFont.id}
            onchange={(event) => setFont(event.currentTarget.value, currentFont.custom)}>
      {#each FONTS as entry (entry.id)}
        <option value={entry.id}>{entry.name}</option>
      {/each}
    </select>
    {#if currentFont.id === "custom"}
      <input type="text" class="input input-sm setting-input" aria-label={m.editorsettings_font_name()} placeholder={m.editorsettings_font_name()}
             value={currentFont.custom}
             onchange={(event) => setFont("custom", event.currentTarget.value)} />
      {#if currentFont.custom && !installed(currentFont.custom)}
        <span class="font-missing">{m.editorsettings_font_missing()}</span>
      {/if}
    {/if}
  </span>
</SettingRow>

<SettingRow id="appearance-font-size" title={m.common_font_size()}
            description={m.editorsettings_font_size_description()}>
  <span class="setting-inline">
    <input type="number" class="input input-sm setting-input setting-number" min={MIN_SIZE} max={MAX_SIZE} step="1"
           aria-label={m.editorsettings_font_size_pixels()} value={currentSize}
           onchange={(event) => {
             const field = event.currentTarget;
             const next = field.value.trim() === "" ? NaN : Number(field.value);
             if (Number.isFinite(next)) setSize(next);
             field.value = String(size());
           }} />
    <span>px</span>
  </span>
</SettingRow>

<SettingRow id="appearance-line-height" title={m.common_line_height()}>
  <select class="select setting-select" aria-label={m.common_line_height()} value={currentLineHeight}
          onchange={(event) => setLineHeight(event.currentTarget.value)}>
    <option value="compact">{m.editorsettings_line_height_compact()}</option>
    <option value="normal">{m.editorsettings_line_height_normal()}</option>
    <option value="wide">{m.editorsettings_line_height_wide()}</option>
  </select>
</SettingRow>

<SettingRow id="editor-keys" title={m.common_key_bindings()}>
  <select class="select setting-select" aria-label={m.editorsettings_keys_label()} value={keys}
          onchange={(event) => onkeys?.(event.currentTarget.value)}>
    <option value="default">{m.editorsettings_keys_standard()}</option>
    <option value="vim">Vim</option>
    <option value="emacs">Emacs</option>
  </select>
</SettingRow>

<SettingRow id="editor-completion" title={m.common_autocomplete()}
            description={m.editorsettings_completion_description()}>
  <button type="button" role="switch" class="switch" aria-checked={completion} aria-label={m.common_autocomplete()}
          data-state={completion ? "checked" : "unchecked"} onclick={() => oncompletion?.(!completion)}>
    <span class="switch-thumb" data-state={completion ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<SettingRow id="editor-brackets" title={m.common_close_brackets()}
            description={m.editorsettings_brackets_description()}>
  <button type="button" role="switch" class="switch" aria-checked={brackets} aria-label={m.common_close_brackets()}
          data-state={brackets ? "checked" : "unchecked"} onclick={() => onbrackets?.(!brackets)}>
    <span class="switch-thumb" data-state={brackets ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<SettingRow id="editor-proofing" title={m.common_grammar_and_spelling()} description={proofingDescription}>
  <select class="select setting-select" aria-label={m.common_grammar_and_spelling()} value={proofing}
          onchange={(event) => onproofing?.(event.currentTarget.value)}>
    <option value="off">{m.editorsettings_proofing_off()}</option>
    <option value="browser">{m.common_browser()}</option>
    <option value="harper">Harper</option>
  </select>
</SettingRow>

{#if proofing === "harper"}
  <SettingRow id="editor-dialect" title={m.common_english_dialect()}>
    <select class="select setting-select" aria-label={m.common_english_dialect()} value={dialect}
            onchange={(event) => ondialect?.(event.currentTarget.value)}>
      <option value="american">{m.editorsettings_dialect_american()}</option>
      <option value="british">{m.editorsettings_dialect_british()}</option>
      <option value="canadian">{m.editorsettings_dialect_canadian()}</option>
      <option value="australian">{m.editorsettings_dialect_australian()}</option>
      <option value="indian">{m.editorsettings_dialect_indian()}</option>
    </select>
  </SettingRow>
{/if}

<SettingRow id="editor-check" title={m.common_code_check()}
            description={m.editorsettings_check_description()}>
  <button type="button" role="switch" class="switch" aria-checked={check} aria-label={m.common_code_check()}
          data-state={check ? "checked" : "unchecked"} onclick={() => oncheck?.(!check)}>
    <span class="switch-thumb" data-state={check ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<SettingRow id="editor-visual" title={m.common_visual_mode()}
            description={m.editorsettings_visual_description()}>
  <button type="button" role="switch" class="switch" aria-checked={visual} aria-label={m.common_visual_mode()}
          data-state={visual ? "checked" : "unchecked"} onclick={() => onvisual?.(!visual)}>
    <span class="switch-thumb" data-state={visual ? "checked" : "unchecked"}></span>
  </button>
</SettingRow>

<!-- The table is shown here and only here: `?` and the keyboard button both
     lead to this page. It comes last so it never pushes the switches above
     out of view. Stacked, because a table is not a control that fits beside
     a sentence. -->
<SettingRow id="editor-shortcuts" stacked title={m.common_keyboard_shortcuts()}
            description={m.editorsettings_shortcuts_description()}>
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
