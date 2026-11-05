<script>
  // The one time a token is shown. It lives in the parent's component state
  // and nowhere else; nothing here writes it to storage.
  import { m } from "../../../lib/paraglide/messages.js";
  let { token, oncopy, ondone } = $props();
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
</script>

<div class="share-reveal">
  <p class="share-warning" role="alert">{m.tokenreveal_warning()}</p>
  <label class="share-field panel-meta">{m.tokenreveal_token()}
    <input class="input share-input" type="text" readonly aria-label={m.common_agent_token()} value={token} onclick={select} />
  </label>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncopy(token)}>{m.tokenreveal_copy_token()}</button>
  <label class="share-field panel-meta">Claude Code
    <textarea class="input share-input share-config" readonly rows="2" aria-label={m.tokenreveal_commands_aria()} value={commands} onclick={select}></textarea>
    <span>{m.tokenreveal_commands_help()}</span>
  </label>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncopy(commands)}>{m.tokenreveal_copy_commands()}</button>
  <label class="share-field panel-meta">{m.tokenreveal_other_clients()}
    <textarea class="input share-input share-config" readonly rows="5" aria-label={m.tokenreveal_config_aria()} value={config} onclick={select}></textarea>
  </label>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncopy(config)}>{m.tokenreveal_copy_config()}</button>
  <p class="panel-meta">{m.tokenreveal_api_hint({ endpoint: "POST /api/documents/{slug}/tools/{name}" })}</p>
  <p class="panel-meta"><a href="https://librepaper.org/agents.html#agent-tokens" target="_blank" rel="noreferrer">{m.tokenreveal_agent_setup()}</a></p>
  <div class="share-form-actions">
    <button type="button" class="btn btn-sm lp-control-brand" onclick={ondone}>{m.common_done()}</button>
  </div>
</div>

<style>
  .share-reveal { display: grid; gap: calc(var(--spacing) * 2); padding-block: calc(var(--spacing) * 2); }
  .share-warning { margin: 0; font-size: 13px; font-weight: 500; }
  .share-field { display: grid; gap: var(--spacing); }
  .share-field > span { font-size: 11px; }
  .share-input { width: 100%; min-width: 0; max-width: 100%; font-family: var(--font-mono, monospace); font-size: 12px; }
  .share-config { resize: none; white-space: pre; overflow-x: auto; }
  .share-reveal > button { justify-self: start; }
  .share-form-actions { display: flex; justify-content: flex-end; }
  .share-reveal a { color: var(--color-link); text-decoration: underline; }
  /* Phones: the token and commands are read as text that wraps, not scrolled
     sideways, the fields keep 16px, and the buttons are finger-sized. */
  @media (max-width: 767px) {
    .share-input { font-size: 1rem; }
    .share-config { white-space: pre-wrap; overflow-wrap: anywhere; }
    .btn { min-height: 2.75rem; }
  }
</style>
