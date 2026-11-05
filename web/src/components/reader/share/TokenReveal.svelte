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
  // The two blocks are addressed to an agent, not to the user, so they stay
  // English rather than going through the message catalogues.
  const installBlock = [
    "Install the LibrePaper agent skill for every project on this computer by running:",
    "",
    "npx skills add LibrePaper/librepaper --skill librepaper --global",
    "",
    "If that does not work, copy https://github.com/LibrePaper/librepaper/tree/main/skills/librepaper, including its references and scripts folders, into your skills folder. If your sandbox blocks this or the command needs my input, give me the exact command to run in my terminal. When the skill is installed, tell me to start a new session.",
  ].join("\n");
  const connectBlock = $derived([
    "Use the LibrePaper skill to connect to my project with the agent token below. If you do not have that skill, stop and ask me to install it.",
    "",
    "The project is on a LibrePaper server and the token names it, so you need no local folder. The token is a credential: do not repeat it to me or write it into any file in your working directory.",
    "",
    "Connect first, then tell me what you reached and ask what I want done. If your sandbox blocks network or file access, tell me exactly what to allow.",
    "",
    `Token: ${token}`,
  ].join("\n"));
  const select = (event) => event.currentTarget.select();
</script>

<div class="share-reveal">
  <p class="share-warning" role="alert">{m.tokenreveal_warning()}</p>
  <label class="share-field panel-meta">{m.tokenreveal_step_install()}
    <textarea class="input share-input share-config share-block" readonly rows="6" aria-label={m.tokenreveal_install_aria()} value={installBlock} onclick={select}></textarea>
    <span>{m.tokenreveal_step_install_help()}</span>
  </label>
  <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncopy(installBlock)}>{m.tokenreveal_copy_install()}</button>
  <label class="share-field panel-meta">{m.tokenreveal_step_connect()}
    <textarea class="input share-input share-config share-block" readonly rows="7" aria-label={m.tokenreveal_connect_aria()} value={connectBlock} onclick={select}></textarea>
    <span>{m.tokenreveal_step_connect_help()}</span>
  </label>
  <button type="button" class="btn btn-sm lp-control-brand" onclick={() => oncopy(connectBlock)}>{m.tokenreveal_copy_connect()}</button>
  <p class="panel-meta">{m.tokenreveal_chat_note()}</p>
  <details class="share-other">
    <summary>{m.tokenreveal_other_ways()}</summary>
    <div class="share-other-body">
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
    </div>
  </details>
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
  .share-block { white-space: pre-wrap; }
  .share-reveal > button, .share-other-body > button { justify-self: start; }
  .share-other > summary { cursor: pointer; font-size: 12px; color: var(--panel-muted); }
  .share-other-body { display: grid; gap: calc(var(--spacing) * 2); padding-top: calc(var(--spacing) * 2); }
  .share-form-actions { display: flex; justify-content: flex-end; }
  .share-reveal a { color: var(--color-link); text-decoration: underline; }
</style>
