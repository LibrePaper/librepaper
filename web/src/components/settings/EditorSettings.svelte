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
    spellcheck = true,
    onspellcheck,
    check = true,
    oncheck,
    commands = {},
  } = $props();
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

<SettingRow id="editor-spellcheck" title="Spellcheck"
            description="Underline misspelled words in prose, skipping commands, code, math and citation keys. The browser's language and dictionary decide what counts as a word.">
  <button type="button" role="switch" class="switch" aria-checked={spellcheck} aria-label="Spellcheck"
          data-state={spellcheck ? "checked" : "unchecked"} onclick={() => onspellcheck?.(!spellcheck)}>
    <span class="switch-thumb" data-state={spellcheck ? "checked" : "unchecked"}></span>
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
