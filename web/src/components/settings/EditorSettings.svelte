<script>
  // How the source pane behaves for the person at this browser.
  import SettingRow from "./SettingRow.svelte";
  import ShortcutTable from "../ShortcutTable.svelte";

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
    commands = {},
  } = $props();

  const PROOFING_DESCRIPTIONS = {
    off: "Nothing is underlined.",
    browser: "Your browser underlines misspelled words in prose, skipping commands, code, math and citation keys. Its language and dictionary decide what counts as a word.",
    harper: "Harper checks English spelling and grammar in prose. It downloads 7 MB the first time and uses about 300 MB of memory while a document is open. Documents that declare another language get the browser's spellcheck instead.",
  };
  const proofingDescription = $derived(PROOFING_DESCRIPTIONS[proofing] ?? PROOFING_DESCRIPTIONS.browser);
</script>

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
