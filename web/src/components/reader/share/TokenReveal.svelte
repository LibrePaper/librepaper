<script>
  // The one-block copy for a token just made. The token is kept in the parent's
  // component state and nowhere else; nothing here writes it to storage. The
  // owner can copy it again from its row, so this is not the only time it is
  // shown. The External agent tab draws this in place of its token list, and
  // clears it on Done or when the panel closes.
  import { onMount } from "svelte";
  import { m } from "../../../lib/paraglide/messages.js";
  import Icon from "../../Icon.svelte";
  import { connectionInstructions } from "./shared.js";
  let { token, copied = "", copyFallback = "", error = "", oncopy, ondone } = $props();
  const select = (event) => event.currentTarget.select();

  let root = $state(null);
  // Focus starts on the first button, the connection copy, so Enter copies
  // the instructions rather than dismissing the reveal.
  onMount(() => root?.querySelector("button")?.focus());

  const connect = $derived(connectionInstructions(token));
</script>

<div class="token-reveal" bind:this={root}>
  <span class="sr-only" role="status" aria-live="polite">{copied ? m.tokenreveal_copied() : ""}</span>

  <button type="button" class="btn btn-sm token-copy token-copy-wide lp-control-tonal-brand" class:token-copy-failed={copyFallback === connect}
          onclick={() => oncopy(connect)}>
    <Icon name={copied === connect ? "check" : "copy"} size={14} />
    {#if copyFallback === connect}{m.tokenreveal_copy_failed()}{:else if copied === connect}{m.tokenreveal_copied()}{:else}{m.tokenreveal_copy_connect()}{/if}
  </button>
  <p class="token-note">{m.agent_token_warning()}</p>

  {#if error}
    <p class="lp-text-error token-problem" role="alert">{error}</p>
  {/if}
  {#if copyFallback}
    <textarea class="input token-code token-fallback" readonly rows="3" aria-label={m.share_copy_manual_label()} value={copyFallback} onclick={select}></textarea>
  {/if}

  <div class="token-done">
    <button type="button" class="btn btn-sm lp-control-brand" onclick={ondone}>{m.common_done()}</button>
  </div>
</div>

<style>
  .token-reveal { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); min-width: 0; overflow-wrap: anywhere; font-size: 13px; }
  .token-note { margin: 0; font-size: 12px; color: var(--panel-muted); }
  .token-copy { display: flex; align-items: center; justify-content: center; gap: calc(var(--spacing) * 2); width: fit-content; max-width: 100%; }
  .token-copy-wide { width: 100%; }
  /* The panel scrolls and clips, so the ring sits inside the button. */
  .token-copy:focus-visible { outline-offset: -3px; }
  .token-copy-failed { color: var(--color-error-text); }
  .token-problem { margin: 0; }
  .token-fallback { width: 100%; }
  .token-code { width: 100%; min-width: 0; max-width: 100%; box-sizing: border-box; font-family: var(--font-mono, monospace); font-size: 12px; resize: none; white-space: pre-wrap; overflow-wrap: anywhere; }
  .token-done { display: flex; justify-content: flex-end; padding-top: calc(var(--spacing) * 2); }
  /* Phones: the fallback field keeps 16px, and the buttons are finger-sized. */
  @media (max-width: 767px) {
    .token-code { font-size: 1rem; }
    .btn { min-height: 2.75rem; }
  }
</style>
