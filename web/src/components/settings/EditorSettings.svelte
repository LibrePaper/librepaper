<script>
  // How the source pane behaves for the person at this browser.
  import SettingRow from "./SettingRow.svelte";

  let { keys = "default", onkeys } = $props();
</script>

<SettingRow id="editor-keys" title="Keys" description="Standard keys, or the motions and commands of Vim or Emacs, in the source pane.">
  <select class="select setting-select" aria-label="Editor keys" value={keys}
          onchange={(event) => onkeys?.(event.currentTarget.value)}>
    <option value="default">Standard</option>
    <option value="vim">Vim</option>
    <option value="emacs">Emacs</option>
  </select>
</SettingRow>
