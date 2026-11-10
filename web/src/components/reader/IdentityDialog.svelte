<script>
  import { m } from "../../lib/paraglide/messages.js";
  import Modal from "../Modal.svelte";

  let { name, signInHref, oncontinue } = $props();

  let identifying = $state(false);

  export function ask() {
    identifying = true;
  }
</script>

<!-- Signed out, a commenter chooses how to be named before they write. -->
<Modal
  bind:open={identifying}
  title={m.readerd_who_are_you()}
  description={m.readerd_choose_identity()}
  cancelLabel={m.common_sign_in()}
  confirm={{
    label: m.readerd_continue_as({ name }),
    oncancel: () => { identifying = false; location.href = signInHref(); },
    onclick: () => { identifying = false; oncontinue(); },
  }}
></Modal>
