<script>
  // Other connection methods: a discreet disclosure at the foot of the Tokens
  // tab. It offers the MCP setup commands, the MCP config for one token, and the
  // token itself, so it stays available with or without a reveal.
  import { m } from "../../../lib/paraglide/messages.js";
  import Icon from "../../Icon.svelte";
  import IconButton from "../../IconButton.svelte";
  import { displayName } from "./shared.js";

  let { token = "", tokens = [], copied = "", copyFallback = "", oncopy } = $props();

  // Claude Code passes its environment to the servers it starts, so the
  // server is registered once and the token is given at launch. The token is
  // read silently rather than written into the command, so it never lands in
  // shell history.
  const commands = "claude mcp add librepaper --scope user -- librepaper mcp\nread -rs LIBREPAPER_AGENT_TOKEN && export LIBREPAPER_AGENT_TOKEN && claude";
  let chosen = $state("");
  let open = $state(false);
  // The token the methods use: the one just revealed, else the row picked here, else the first.
  const active = $derived(token || tokens.find((item) => item.id === chosen)?.token || tokens[0]?.token || "");
  const config = $derived(`[mcp_servers.librepaper]
command = "librepaper"
args = ["mcp"]
env = { LIBREPAPER_AGENT_TOKEN = "${active}" }`);

  // The rows in Other connection methods: the name shown, the icon's label, and
  // the payload it copies whole.
  const methods = $derived([
    { name: m.tokenreveal_commands_aria(), label: m.tokenreveal_commands_aria(), value: commands },
    ...(active ? [
      { name: m.tokenreveal_config_aria(), label: m.tokenreveal_config_aria(), value: config },
      { name: m.tokenreveal_token(), label: m.tokenreveal_copy_token(), value: active },
    ] : []),
  ]);
</script>

<details class="token-other" bind:open>
  <summary>
    <Icon name={open ? "chevron-down" : "chevron-right"} size={14} />
    {m.tokenreveal_other_ways()}
  </summary>
  <div class="token-other-body">
    <p class="token-help">{m.tokenreveal_mcp_advantage()}</p>
    {#if !token && tokens.length > 1}
      <label class="token-choice"><span>{m.connectionmethods_choose_token()}</span>
        <select class="select" value={chosen || tokens[0].id} onchange={(event) => (chosen = event.currentTarget.value)}>
          {#each tokens as item (item.id)}<option value={item.id}>{displayName("token", item)}</option>{/each}
        </select></label>
    {/if}
    {#if !active}<p class="token-help">{m.connectionmethods_no_token()}</p>{/if}
    <ul class="token-methods">
      {#each methods as method (method.label)}
        <li class="token-method">
          <span>{method.name}</span>
          <IconButton icon={copied === method.value ? "check" : "copy"} label={method.label} title={method.label} tone="plain" size="btn-icon-sm"
                      colour={copyFallback === method.value ? "lp-text-error" : null} onclick={() => oncopy(method.value)} />
        </li>
      {/each}
    </ul>
    <p class="token-help"><a href="https://librepaper.org/agents.html#other-connections" target="_blank" rel="noreferrer">{m.tokenreveal_agent_setup()}</a></p>
  </div>
</details>

<style>
  .token-other { font-size: 13px; margin-top: auto; padding-top: calc(var(--spacing) * 2); min-width: 0; }
  .token-help { margin: 0; font-size: 13px; color: var(--panel-muted); }
  .token-help a { color: var(--color-link); text-decoration: underline; }
  .token-other > summary { display: flex; align-items: center; gap: calc(var(--spacing) * 2); padding-block: calc(var(--spacing) * 1); cursor: pointer; font-size: var(--panel-meta-size); font-weight: 400; color: var(--panel-muted); border-radius: var(--radius-base); list-style: none; }
  .token-other > summary:hover { color: var(--color-text); }
  .token-other > summary::-webkit-details-marker { display: none; }
  .token-other > summary:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  .token-other-body { display: grid; gap: calc(var(--spacing) * 2); min-width: 0; padding-bottom: calc(var(--spacing) * 2); }
  .token-choice { display: grid; gap: var(--spacing); font-size: 13px; }
  .token-choice .select { width: 100%; min-width: 0; }
  .token-methods { margin: 0; padding: 0; list-style: none; }
  .token-method { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); min-width: 0; padding-block: calc(var(--spacing) * 1); }
  .token-method > span { min-width: 0; }
  /* Phones: the disclosure and its select are finger-sized. */
  @media (max-width: 767px) {
    .token-other > summary { min-height: 2.75rem; }
    .token-choice .select { font-size: 1rem; }
  }
</style>
