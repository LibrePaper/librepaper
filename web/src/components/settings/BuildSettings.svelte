<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";
  import { buildersFor, capabilityFor, supportsOperation } from "../../lib/build-catalog.js";
  import { update } from "../../lib/build-preferences.js";

  let { format = "", documentId = "", userId = "anonymous", preferences = {}, onpreferences } = $props();
  const local = $derived(companion.status);
  $effect(() => companion.watch());
  const builders = $derived(buildersFor(format));
  const localBuilders = $derived(builders.filter((entry) => entry.backend.includes("local")));
  // This pane is not itself a question about the companion. Most of what it
  // offers -- which browser engine, which output -- needs nothing local, and
  // opening it to choose pdfLaTeX must not make the browser ask this person
  // to allow the site "access to other apps and services". So it does not
  // probe when it mounts. Choosing a local tool does (`chooseOption`), and so
  // do the buttons below; until then the local rows say only that nobody has
  // looked yet.
  const browserBuilders = $derived(builders.filter((entry) => entry.backend.includes("browser")));
  const selected = $derived(preferences.selection === "tool" ? (preferences.tool === "tex" && preferences.engine ? `${preferences.backend || "browser"}:tex:${preferences.engine}` : optionValue(preferences.backend || "browser", preferences.tool || "")) : "automatic");

  function scope() { return { origin: globalThis.location?.origin || "", user: userId, document: documentId }; }
  function chooseOutput(output) {
    onpreferences?.(update(scope(), format, { output }));
  }
  // "unknown" is nobody having asked yet, not a tool having been found
  // missing. Offering the local rows then is what lets choosing one be the
  // gesture that looks -- greying them out before the question has been put
  // would make the pane impossible to get out of without a probe it is not
  // entitled to make.
  function disabled(entry) {
    if (!entry.backend.includes("local")) return false;
    if (local?.state === "unknown") return false;
    const capability = capabilityFor(local?.capabilities, entry.id);
    return capability?.available !== true || !supportsOperation(local?.capabilities, entry.id, "build", "snapshot");
  }
  function disabledOutput(output) {
    if (preferences.backend !== "local") return format === "markdown" || format === "quarto" ? output !== "html" : false;
    const capability = capabilityFor(local?.capabilities, preferences.tool);
    return !capability || !Array.isArray(capability.outputs) || !capability.outputs.includes(output);
  }
  const statusMessage = $derived(["unreachable", "denied"].includes(local?.state) ? local.instructions : ({ unknown: "The local companion has not been looked for yet.", reachable: "Local companion is running; connect this site.", unauthorized: "Connect this site to use local tools.", incompatible: "Update the local companion to use these tools.", connected: "Local companion connected." })[local?.state] || "");
  function version(entry) { return capabilityFor(local?.capabilities, entry.id)?.version; }
  function engineLabel(engine) { return engine === "pdflatex" ? "pdfLaTeX" : engine === "xelatex" ? "XeLaTeX" : "LuaLaTeX"; }
  const savedMissing = $derived(preferences.selection === "tool" && preferences.tool && !builders.some((entry) => entry.id === preferences.tool) ? preferences.tool : "");
  // What the chosen builder can produce. A local tool answers for itself; in
  // the browser only Typst offers the paged output beside the flow one.
  function supportedOutputs(backend, tool) {
    if (backend === "local") return capabilityFor(local?.capabilities, tool)?.outputs || [];
    return format === "typst" ? ["pdf", "html"] : ["html"];
  }
  const outputChoices = $derived.by(() => {
    const supported = supportedOutputs(preferences.backend, preferences.tool);
    const choices = [...supported];
    if (preferences.output && !choices.includes(preferences.output)) choices.unshift(preferences.output);
    return choices;
  });
  function optionValue(backend, id) { return `${backend}:${id}`; }
  function chooseOption(value) {
    if (value === "automatic") return onpreferences?.(update(scope(), format, { selection: "automatic", backend: "auto" }));
    const [backend, id, ...rest] = value.split(":");
    const engine = id === "tex" ? rest[0] : "";
    const tool = id;
    // HTML is every format's default output, so a tool starts on HTML unless
    // it is one that cannot produce a flow page at all.
    const outputs = supportedOutputs(backend, tool);
    const output = !outputs.length || outputs.includes("html") ? "html" : outputs[0];
    const next = update(scope(), format, { selection: "tool", backend, tool, output, ...(engine ? { engine } : {}) });
    onpreferences?.(next);
    // Asking for a tool on this computer is asking for the local app: this is
    // the moment the pane may reach loopback, and the capabilities that come
    // back fill in the version and the outputs this tool really has.
    if (backend === "local") void localBridge.probe({ force: true });
  }
  async function rescan() { try { await localBridge.capabilities({ rescan: true }); } catch { /* status explains failure */ } }
  async function connect() { try { await localBridge.connectApp(); } catch { /* local status carries instructions */ } }
</script>

<SettingRow id="render-tool" title={format === "latex" ? "Compiler" : "Build tool"} description="Build choices belong to this browser and user. Collaborators cannot change them.">
  <select class="select setting-select" aria-label={format === "latex" ? "Compiler" : "Build tool"} value={selected}
          onchange={(event) => {
            chooseOption(event.currentTarget.value);
          }}>
    <option value="automatic">Automatic</option>
    {#if browserBuilders.length}<optgroup label="Browser">
      {#each browserBuilders as entry (entry.id)}
        {#if entry.engines.length}
          {#each entry.engines as engine}<option value={`browser:${entry.id}:${engine}`}>{engineLabel(engine)}</option>{/each}
        {:else}<option value={optionValue("browser", entry.id)}>{entry.label}</option>{/if}
      {/each}
    </optgroup>{/if}
    {#if localBuilders.length}<optgroup label="Local companion">
      {#each localBuilders as entry (entry.id)}
        <option value={optionValue("local", entry.id)} disabled={disabled(entry)}>{entry.label}{version(entry) ? ` (${version(entry)})` : ""}{disabled(entry) ? " (unavailable)" : ""}</option>
      {/each}
    </optgroup>{/if}
    {#if savedMissing}<option value={`local:${savedMissing}`} disabled>{savedMissing} (unavailable)</option>{/if}
  </select>
</SettingRow>

<!-- LaTeX has no local builder to configure, and nothing else about it is
     local either, so this pane says nothing about the companion for it. -->
{#if format !== "latex"}<SettingRow title="Local tools" description="Refresh installed tools.">
  <span class="setting-description" role="status">{statusMessage}</span>
  <!-- Nothing here can open the companion; the status line says how to start
       it. Before anybody has looked, the only thing to offer is the looking. -->
  {#if ["unreachable", "denied", "unauthorized", "reachable"].includes(local?.state)}<button type="button" class="btn btn-sm lp-control-brand" onclick={connect}>Connect</button>{/if}
  {#if local?.state !== "connected"}<a class="btn btn-sm lp-control-outline" href="https://github.com/LibrePaper/librepaper/releases/latest" target="_blank" rel="noreferrer">Install companion</a>{/if}
  <button type="button" class="btn btn-sm lp-control-outline" disabled={local?.state !== "connected"} onclick={rescan}>Rescan</button>
</SettingRow>{/if}

{#if ["typst", "quarto", "markdown"].includes(format) && outputChoices.length > 1}
  <SettingRow id="render-output" title="Output" description="Choose the preview or export format for this browser.">
    <select class="select setting-select" aria-label="Build output" value={preferences.output || "html"} onchange={(event) => chooseOutput(event.currentTarget.value)}>
      {#each outputChoices as output}<option value={output} disabled={disabledOutput(output)}>{output.toUpperCase()}</option>{/each}
    </select>
  </SettingRow>
{/if}
