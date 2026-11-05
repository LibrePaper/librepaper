<script>
  // The connection step for a token just made. The token is kept in the parent's
  // component state and nowhere else; nothing here writes it to storage. The
  // owner can copy it again from its row, so this is not the only time it is
  // shown. The Agent panel draws this under its install section in place of its
  // toolbar and list, and clears it on Done or when the panel closes.
  import { onMount } from "svelte";
  import { m } from "../../../lib/paraglide/messages.js";
  import Icon from "../../Icon.svelte";
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
  // What the screen shows until the token is revealed. The copy button still
  // copies `config`, which holds the real token.
  const maskedConfig = $derived(config.split(token).join("lpt_..."));
  const select = (event) => event.currentTarget.select();

  let root = $state(null);
  let otherOpen = $state(false);
  let revealed = $state(false);
  // Focus starts on the first button, the connection copy, so Enter copies
  // the instructions rather than dismissing the reveal.
  onMount(() => root?.querySelector("button")?.focus());
</script>

{#snippet copyButton(label, value, tone, wide)}
  <button type="button" class="btn btn-sm token-copy {tone === 'brand' ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
          class:token-copy-wide={wide} class:token-copy-failed={copyFallback === value} onclick={() => oncopy(value)}>
    <Icon name={copied === value ? "check" : "copy"} size={14} />
    {#if copyFallback === value}{m.tokenreveal_copy_failed()}{:else if copied === value}{m.tokenreveal_copied()}{:else}{label}{/if}
  </button>
{/snippet}

<div class="token-reveal" bind:this={root}>
  <span class="sr-only" role="status" aria-live="polite">{copied ? m.tokenreveal_copied() : ""}</span>

  <section class="token-section">
    <div class="token-step-head">
      <span class="token-number" aria-hidden="true">2</span>
      <div class="token-step-text">
        <h3 class="token-step-title">{m.tokenreveal_step_connect()}</h3>
        <p class="token-help">{m.tokenreveal_step_connect_help()}</p>
      </div>
    </div>
    {@render copyButton(m.tokenreveal_copy_connect(), connectionInstructions(token), "brand", true)}
    <p class="token-note">{m.tokenreveal_connect_note()}</p>
  </section>

  <p class="token-warning">
    <Icon name="shield-check" size={16} />
    <span>{m.tokenreveal_warning()}</span>
  </p>

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

      <label class="token-field">Claude Code
        <textarea class="input token-code" readonly rows="4" aria-label={m.tokenreveal_commands_aria()} value={commands} onclick={select}></textarea>
        <span class="token-help">{m.tokenreveal_commands_help()}</span>
      </label>
      {@render copyButton(m.tokenreveal_copy_commands(), commands, "tonal", false)}

      <label class="token-field">{m.tokenreveal_other_clients()}
        <textarea class="input token-code" readonly rows="5" aria-label={m.tokenreveal_config_aria()} value={revealed ? config : maskedConfig} onclick={select}></textarea>
      </label>
      {@render copyButton(m.tokenreveal_copy_config(), config, "tonal", false)}

      <div class="token-field">
        <h4 class="token-label">{m.tokenreveal_token()}</h4>
        <div class="token-row">
          <button type="button" class="btn btn-sm lp-control-tonal-brand" aria-expanded={revealed} onclick={() => (revealed = !revealed)}>
            {revealed ? m.tokenreveal_hide_token() : m.tokenreveal_reveal_token()}
          </button>
          {@render copyButton(m.tokenreveal_copy_token(), token, "tonal", false)}
        </div>
        {#if revealed}
          <input class="input token-code" type="text" readonly aria-label={m.common_agent_token()} value={token} onclick={select} />
        {/if}
      </div>

      <p class="token-help">{m.tokenreveal_api_hint({ endpoint: "POST /api/documents/{slug}/tools/{name}" })}</p>
      <p class="token-help"><a href="https://librepaper.org/agents.html#agent-tokens" target="_blank" rel="noreferrer">{m.tokenreveal_agent_setup()}</a></p>
    </div>
  </details>

  <div class="token-done">
    <button type="button" class="btn btn-sm lp-control-brand" onclick={ondone}>{m.common_done()}</button>
  </div>
</div>

<style>
  .token-reveal { display: flex; flex-direction: column; min-width: 0; overflow-wrap: anywhere; font-size: 13px; }
  .token-section { padding-block: calc(var(--spacing) * 3); border-top: 1px solid var(--color-divider); }
  .token-section:first-of-type { padding-top: 0; border-top: 0; }
  .token-step-head { display: flex; align-items: flex-start; gap: calc(var(--spacing) * 3); margin-bottom: calc(var(--spacing) * 3); }
  .token-step-text { min-width: 0; }
  .token-step-title { margin: 0; font-size: 14px; font-weight: 500; }
  .token-help { margin: calc(var(--spacing) * 1) 0 0; font-size: 13px; color: var(--panel-muted); }
  .token-note { margin: calc(var(--spacing) * 2) 0 0; font-size: 12px; color: var(--panel-muted); }
  .token-copy { display: flex; align-items: center; justify-content: center; gap: calc(var(--spacing) * 2); width: fit-content; max-width: 100%; }
  .token-copy-wide { width: 100%; }
  /* The panel scrolls and clips, so the ring sits inside the button. */
  .token-copy:focus-visible { outline-offset: -3px; }
  .token-copy-failed { color: var(--color-error-text); }
  .token-warning { display: flex; align-items: flex-start; gap: calc(var(--spacing) * 2); margin: 0; padding: calc(var(--spacing) * 3); border-radius: var(--radius-base); background: var(--color-row-hover); font-size: 12px; }
  .token-warning > span { min-width: 0; }
  .token-problem { margin: calc(var(--spacing) * 2) 0 0; }
  .token-fallback { width: 100%; margin-top: calc(var(--spacing) * 2); }
  .token-other { margin-top: calc(var(--spacing) * 3); padding-block: calc(var(--spacing) * 3); border-top: 1px solid var(--color-divider); }
  .token-other > summary { display: flex; align-items: center; gap: calc(var(--spacing) * 2); cursor: pointer; font-size: 13px; font-weight: 500; border-radius: var(--radius-base); list-style: none; }
  .token-other > summary::-webkit-details-marker { display: none; }
  .token-other > summary:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  .token-other-body { display: grid; gap: calc(var(--spacing) * 3); min-width: 0; padding-top: calc(var(--spacing) * 3); }
  .token-field { display: grid; gap: var(--spacing); min-width: 0; font-size: 12px; color: var(--panel-muted); }
  .token-label { margin: 0; font-size: 12px; font-weight: 500; color: var(--color-text); }
  .token-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); }
  .token-code { width: 100%; min-width: 0; max-width: 100%; box-sizing: border-box; font-family: var(--font-mono, monospace); font-size: 12px; resize: none; white-space: pre-wrap; overflow-wrap: anywhere; }
  .token-done { display: flex; justify-content: flex-end; padding-top: calc(var(--spacing) * 3); }
  .token-reveal a { color: var(--color-link); text-decoration: underline; }
</style>
