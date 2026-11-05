<script>
  // The connection step for a token just made. The token is kept in the parent's
  // component state and nowhere else; nothing here writes it to storage. The
  // owner can copy it again from its row, so this is not the only time it is
  // shown. The Agent panel draws this under its connect step in place of its
  // token list, and clears it on Done or when the panel closes.
  import { onMount } from "svelte";
  import { m } from "../../../lib/paraglide/messages.js";
  import Icon from "../../Icon.svelte";
  import IconButton from "../../IconButton.svelte";
  import { connectionInstructions } from "./shared.js";
  let { token, copied = "", copyFallback = "", error = "", oncopy, ondone } = $props();
  // Claude Code passes its environment to the servers it starts, so the
  // server is registered once and the token is given at launch. The token is
  // read silently rather than written into the command, so it never lands in
  // shell history.
  const commands = "claude mcp add librepaper --scope user -- librepaper mcp\nread -rs LIBREPAPER_AGENT_TOKEN && export LIBREPAPER_AGENT_TOKEN && claude";
  const config = $derived(`[mcp_servers.librepaper]
command = "librepaper"
args = ["mcp"]
env = { LIBREPAPER_AGENT_TOKEN = "${token}" }`);
  const select = (event) => event.currentTarget.select();

  let root = $state(null);
  let otherOpen = $state(false);
  // Focus starts on the first button, the connection copy, so Enter copies
  // the instructions rather than dismissing the reveal.
  onMount(() => root?.querySelector("button")?.focus());

  // The rows in Other connection methods: the name shown, the icon's label, and
  // the payload it copies whole.
  const connect = $derived(connectionInstructions(token));
  const methods = $derived([
    { name: m.tokenreveal_commands_aria(), label: m.tokenreveal_commands_aria(), value: commands },
    { name: m.tokenreveal_config_aria(), label: m.tokenreveal_config_aria(), value: config },
    { name: m.tokenreveal_token(), label: m.tokenreveal_copy_token(), value: token },
  ]);
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

  <details class="token-other" bind:open={otherOpen}>
    <summary>
      <Icon name={otherOpen ? "chevron-down" : "chevron-right"} size={16} />
      {m.tokenreveal_other_ways()}
    </summary>
    <div class="token-other-body">
      <p class="token-help">{m.tokenreveal_mcp_advantage()}</p>
      <ul class="token-methods">
        {#each methods as method (method.label)}
          <li class="token-method">
            <span>{method.name}</span>
            <IconButton icon={copied === method.value ? "check" : "copy"} label={method.label} title={method.label} tone="plain" size="btn-icon-sm"
                        colour={copyFallback === method.value ? "lp-text-error" : null} onclick={() => oncopy(method.value)} />
          </li>
        {/each}
      </ul>
      <p class="token-help"><a href="https://librepaper.org/agents.html#agent-tokens" target="_blank" rel="noreferrer">{m.tokenreveal_agent_setup()}</a></p>
    </div>
  </details>

  <div class="token-done">
    <button type="button" class="btn btn-sm lp-control-brand" onclick={ondone}>{m.common_done()}</button>
  </div>
</div>

<style>
  .token-reveal { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); min-width: 0; overflow-wrap: anywhere; font-size: 13px; }
  .token-help { margin: 0; font-size: 13px; color: var(--panel-muted); }
  .token-note { margin: 0; font-size: 12px; color: var(--panel-muted); }
  .token-copy { display: flex; align-items: center; justify-content: center; gap: calc(var(--spacing) * 2); width: fit-content; max-width: 100%; }
  .token-copy-wide { width: 100%; }
  /* The panel scrolls and clips, so the ring sits inside the button. */
  .token-copy:focus-visible { outline-offset: -3px; }
  .token-copy-failed { color: var(--color-error-text); }
  .token-problem { margin: 0; }
  .token-fallback { width: 100%; }
  .token-code { width: 100%; min-width: 0; max-width: 100%; box-sizing: border-box; font-family: var(--font-mono, monospace); font-size: 12px; resize: none; white-space: pre-wrap; overflow-wrap: anywhere; }
  .token-other { padding-top: calc(var(--spacing) * 1); border-top: 1px solid var(--color-divider); }
  .token-other > summary { display: flex; align-items: center; gap: calc(var(--spacing) * 2); padding-block: calc(var(--spacing) * 2); cursor: pointer; font-size: 13px; font-weight: 500; border-radius: var(--radius-base); list-style: none; }
  .token-other > summary::-webkit-details-marker { display: none; }
  .token-other > summary:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  .token-other-body { display: grid; gap: calc(var(--spacing) * 2); min-width: 0; padding-bottom: calc(var(--spacing) * 2); }
  .token-methods { margin: 0; padding: 0; list-style: none; }
  .token-method { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); min-width: 0; padding-block: calc(var(--spacing) * 1); }
  .token-method > span { min-width: 0; }
  .token-done { display: flex; justify-content: flex-end; padding-top: calc(var(--spacing) * 2); }
  .token-reveal a { color: var(--color-link); text-decoration: underline; }
</style>
