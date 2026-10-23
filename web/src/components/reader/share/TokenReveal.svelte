<script>
  // The one time a token is shown. It lives in the parent's component state
  // and nowhere else; nothing here writes it to storage.
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
  <p class="share-warning" role="alert">Copy this token now. You won't be able to view it again.</p>
  <label class="share-field panel-meta">Token
    <input class="input share-input" type="text" readonly aria-label="Agent token" value={token} onclick={select} />
  </label>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncopy(token)}>Copy token</button>
  <label class="share-field panel-meta">Claude Code
    <textarea class="input share-input share-config" readonly rows="2" aria-label="Claude Code commands" value={commands} onclick={select}></textarea>
    <span>Run the first line once. The second asks for the token: paste it, press Enter, and Claude starts.</span>
  </label>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncopy(commands)}>Copy commands</button>
  <label class="share-field panel-meta">Other MCP clients
    <textarea class="input share-input share-config" readonly rows="5" aria-label="MCP configuration" value={config} onclick={select}></textarea>
  </label>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncopy(config)}>Copy configuration</button>
  <p class="panel-meta">Agents without MCP can call POST /api/documents/{"{slug}"}/tools/{"{name}"} with the token as a Bearer header.</p>
  <p class="panel-meta"><a href="https://librepaper.org/agents.html#agent-tokens" target="_blank" rel="noreferrer">Agent setup</a></p>
  <div class="share-form-actions">
    <button type="button" class="btn btn-sm lp-control-brand" onclick={ondone}>Done</button>
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
</style>
