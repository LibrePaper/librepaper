<script>
  // How the source pane behaves for the person at this browser.
  import SettingRow from "./SettingRow.svelte";
  import ShortcutTable from "../ShortcutTable.svelte";

  let { keys = "default", onkeys, commands = {} } = $props();
</script>

<SettingRow id="editor-keys" title="Keys" description="Choose standard, Vim or Emacs keys for the source pane.">
  <select class="select setting-select" aria-label="Editor keys" value={keys}
          onchange={(event) => onkeys?.(event.currentTarget.value)}>
    <option value="default">Standard</option>
    <option value="vim">Vim</option>
    <option value="emacs">Emacs</option>
  </select>
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
