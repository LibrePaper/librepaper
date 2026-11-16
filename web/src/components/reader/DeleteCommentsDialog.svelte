<script>
  import { m } from "../../lib/paraglide/messages.js";
  import Modal from "../Modal.svelte";

  let { ondelete } = $props();

  let deleting = $state(false);
  let pendingDelete = $state([]);

  export function ask(items) {
    if (!items.length) return;
    pendingDelete = items;
    deleting = true;
  }

  function confirmDelete() {
    const items = pendingDelete;
    pendingDelete = [];
    deleting = false;
    for (const comment of items) ondelete(comment);
  }
</script>

<!-- Deleting a thread cannot be undone, so it is confirmed. -->
<Modal
  bind:open={deleting}
  title={pendingDelete.length > 1 ? m.readerd_delete_many_comments_title({ count: pendingDelete.length }) : m.readerd_delete_comment_title()}
  description={pendingDelete.length > 1
    ? m.readerd_delete_many_comments_body()
    : m.readerd_delete_comment_body()}
  confirm={{ label: m.common_delete(), tone: "error", onclick: confirmDelete }}
></Modal>
