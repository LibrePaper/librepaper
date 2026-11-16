<script>
  import { m } from "../../lib/paraglide/messages.js";
  import Modal from "../Modal.svelte";

  let { onclear } = $props();

  let clearingChat = $state(false);

  export function ask() {
    clearingChat = true;
  }

  function confirmClearChat() {
    clearingChat = false;
    onclear();
  }
</script>

<!-- Clearing the chat deletes it for everyone, so it is confirmed. -->
<Modal
  bind:open={clearingChat}
  title={m.readerd_delete_chat_title()}
  description={m.readerd_delete_chat_body()}
  confirm={{ label: m.readerd_delete_history(), tone: "error", onclick: confirmClearChat }}
></Modal>
