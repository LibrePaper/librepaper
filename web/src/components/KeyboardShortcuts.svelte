<script>
  import Modal from "./Modal.svelte";
  import ShortcutTable from "./ShortcutTable.svelte";
  import { APPLE } from "../lib/commands.js";

  // The table behind `?` and behind the button in the bar. The dialog is all
  // this file is: the table itself is a component, because the settings show
  // the same one beside the choice of editor keys.
  let { open = $bindable(false), context = {}, modalEditor = false, apple = APPLE } = $props();
</script>

<Modal bind:open title="Keyboard shortcuts" wide
       description="LibrePaper leaves the browser's own shortcuts alone. These are the application's.">
  <ShortcutTable {context} {modalEditor} {apple} />
</Modal>
