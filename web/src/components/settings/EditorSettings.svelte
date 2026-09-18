<script>
  // How the source pane behaves for the person at this browser.
  import SettingRow from "./SettingRow.svelte";
  import ShortcutTable from "../ShortcutTable.svelte";

  let { keys = "default", onkeys, commands = {} } = $props();

  const modalEditor = $derived(keys === "vim" || keys === "emacs");
</script>

<SettingRow id="editor-keys" title="Keys" description="Standard keys, or the motions and commands of Vim or Emacs, in the source pane.">
  <select class="select setting-select" aria-label="Editor keys" value={keys}
          onchange={(event) => onkeys?.(event.currentTarget.value)}>
    <option value="default">Standard</option>
    <option value="vim">Vim</option>
    <option value="emacs">Emacs</option>
  </select>
</SettingRow>

<!-- The same table `?` opens, under the choice that changes half of it: the
     row above decides who owns the editing keys, and reading what they are is
     the next thing anybody does after changing it. Stacked, because a table is
     not a control that fits beside a sentence. -->
<SettingRow id="editor-shortcuts" stacked title="Keyboard shortcuts"
            description="What the keyboard does in this workspace. Press ? at any time to see this without opening the settings.">
  <ShortcutTable context={commands} {modalEditor} />
</SettingRow>
