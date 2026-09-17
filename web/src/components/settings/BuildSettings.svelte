<script>
  import { Switch } from "@skeletonlabs/skeleton-svelte";
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
  const selected = $derived(preferences.selection === "tool" ? (preferences.preset ? `local:preset:${preferences.preset}` : preferences.tool === "tex" && preferences.engine ? `${preferences.backend || "browser"}:tex:${preferences.engine}` : optionValue(preferences.backend || "browser", preferences.tool || "")) : "automatic");

  function scope() { return { origin: globalThis.location?.origin || "", user: userId, document: documentId }; }
  function chooseOutput(output) {
    onpreferences?.(update(scope(), format, { output }));
  }
  function chooseProfile(profile) { onpreferences?.(update(scope(), format, { profile: profile.trim() || null })); }
  function chooseParameters(text) {
    try { const parameters = text.trim() ? JSON.parse(text) : {}; if (!parameters || Array.isArray(parameters) || typeof parameters !== "object") return; onpreferences?.(update(scope(), format, { parameters })); } catch { /* leave the last valid map */ }
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
  const statusMessage = $derived(({ unknown: "The local companion has not been looked for yet.", unreachable: "Local companion unavailable.", denied: "Local network access was blocked by the browser.", reachable: "Local companion is running; connect this document.", unauthorized: "Connect this document to use local tools.", incompatible: "Update the local companion to use these tools.", connected: "Local companion connected." })[local?.state] || "");
  function version(entry) { return capabilityFor(local?.capabilities, entry.id)?.version; }
  function engineLabel(engine) { return engine === "pdflatex" ? "pdfLaTeX" : engine === "xelatex" ? "XeLaTeX" : "LuaLaTeX"; }
  const savedMissing = $derived(preferences.selection === "tool" && preferences.tool && !builders.some((entry) => entry.id === preferences.tool) ? preferences.tool : "");
  const selectedPreset = $derived(preferences.preset ? presets.find((item) => item.id === preferences.preset) : null);
  const presetSchema = $derived(Object.entries(selectedPreset?.option_schema || {}).map(([name, schema]) => ({ name, ...schema })));
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
  function setPresetOption(name, value) {
    if (!selectedPreset || !presetSchema.some((item) => item.name === name)) return;
    onpreferences?.(update(scope(), format, { options: { ...(preferences.options || {}), [name]: value } }));
  }
  function optionValue(backend, id) { return `${backend}:${id}`; }
  function chooseOption(value) {
    if (value === "automatic") return onpreferences?.(update(scope(), format, { selection: "automatic", backend: "auto" }));
    const [backend, id, ...rest] = value.split(":");
    const preset = id === "preset" ? rest.join(":") : "";
    const engine = id === "tex" ? rest[0] : "";
    const entry = preset && presets.find((item) => item.id === preset);
    const tool = preset ? (entry?.base_adapter || "") : id;
    // HTML is every format's default output, so a tool starts on HTML unless
    // it is one that cannot produce a flow page at all.
    const outputs = supportedOutputs(backend, tool);
    const output = !outputs.length || outputs.includes("html") ? "html" : outputs[0];
    const next = update(scope(), format, { selection: "tool", backend, tool, output, ...(engine ? { engine } : {}), ...(preset ? { preset } : {}) });
    onpreferences?.(next);
    // Asking for a tool on this computer is asking for the local app: this is
    // the moment the pane may reach loopback, and the capabilities that come
    // back fill in the version and the outputs this tool really has.
    if (backend === "local") void localBridge.probe({ force: true });
  }
  async function rescan() { try { await localBridge.capabilities({ rescan: true }); } catch { /* status explains failure */ } }
  async function connect() { try { await localBridge.connectViaApp(); } catch { /* local status carries instructions */ } }
  const presets = $derived.by(() => {
    const found = Array.isArray(local?.capabilities?.presets) ? [...local.capabilities.presets] : [];
    const compatible = found.filter((item) => (!Array.isArray(item.source_formats) || item.source_formats.includes(format)) && (!item.base_adapter || builders.some((entry) => entry.id === item.base_adapter)));
    if (preferences.preset && !compatible.some((item) => item.id === preferences.preset)) {
      compatible.push({ id: preferences.preset, name: `${preferences.preset} (unavailable)`, available: false });
    }
    return compatible.map((preset) => ({ ...preset, available: preset.available !== false && local?.state === "connected" && capabilityFor(local?.capabilities, preset.base_adapter)?.available === true }));
  });
</script>

<SettingRow id="build-tool" title="Build tool" description="Build choices belong to this browser and user. Collaborators cannot change them.">
  <select class="select setting-select" aria-label="Build tool" value={selected}
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
    {#if presets.length}<optgroup label="Custom presets">
      {#each presets as preset (preset.id)}<option value={`local:preset:${preset.id}`} disabled={preset.available === false}>{preset.display_name || preset.name || preset.id}</option>{/each}
    </optgroup>{/if}
    {#if savedMissing}<option value={`local:${savedMissing}`} disabled>{savedMissing} (unavailable)</option>{/if}
  </select>
</SettingRow>

<!-- LaTeX has no local builder to configure, and nothing else about it is
     local either, so this pane says nothing about the companion for it. -->
{#if format !== "latex"}<SettingRow title="Local tools" description="Refresh installed tools and companion presets.">
  <span class="setting-description" role="status">{statusMessage}</span>
  <!-- Launching the app is an answer to "it is not running". Before anybody
       has looked, the only thing to offer is the looking. -->
  {#if ["unreachable", "denied"].includes(local?.state)}<button type="button" class="btn btn-sm preset-filled-primary-500" onclick={connect}>Open companion</button>{/if}
  {#if ["unauthorized", "reachable"].includes(local?.state)}<button type="button" class="btn btn-sm preset-filled-primary-500" onclick={connect}>Connect</button>{/if}
  {#if local?.state !== "connected"}<a class="btn btn-sm preset-outlined-surface-300-700" href="https://github.com/LibrePaper/librepaper/releases/latest" target="_blank" rel="noreferrer">Install companion</a>{/if}
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.retry()}>{local?.state === "unknown" ? "Look for it" : "Retry"}</button>
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={local?.state !== "connected"} onclick={rescan}>Rescan</button>
</SettingRow>{/if}

{#if selectedPreset && presetSchema.length}
  <fieldset class="setting-group" disabled={selectedPreset.available === false}>
    <legend>Preset options</legend>
    {#each presetSchema as option (option.name)}
      <SettingRow title={option.label || option.name} description={option.description || ""}>
        {#if option.kind === "boolean"}
          <!-- The row's title is the visible name, so the switch's own label
               is there for a screen reader alone. -->
          <Switch checked={Boolean(preferences.options?.[option.name])}
                  onCheckedChange={({ checked }) => setPresetOption(option.name, checked)}>
            <Switch.Label class="sr-only">{option.label || option.name}</Switch.Label>
            <Switch.Control class="switch"><Switch.Thumb class="switch-thumb" /></Switch.Control>
            <Switch.HiddenInput />
          </Switch>
        {:else if option.kind === "enum" && Array.isArray(option.values)}
          <select class="select setting-select" aria-label={option.label || option.name} value={preferences.options?.[option.name] ?? option.values[0]} onchange={(event) => setPresetOption(option.name, event.currentTarget.value)}>
            {#each option.values as value}<option value={value}>{value}</option>{/each}
          </select>
        {:else if option.kind === "string"}
          <input class="input input-sm setting-input" type="text" aria-label={option.label || option.name} value={preferences.options?.[option.name] || ""} onchange={(event) => setPresetOption(option.name, event.currentTarget.value)} />
        {/if}
      </SettingRow>
    {/each}
  </fieldset>
{/if}

{#if ["typst", "quarto", "markdown"].includes(format) && outputChoices.length > 1}
  <SettingRow id="build-output" title="Output" description="Choose the preview or export format for this browser.">
    <select class="select setting-select" aria-label="Build output" value={preferences.output || "html"} onchange={(event) => chooseOutput(event.currentTarget.value)}>
      {#each outputChoices as output}<option value={output} disabled={disabledOutput(output)}>{output.toUpperCase()}</option>{/each}
    </select>
  </SettingRow>
{/if}

{#if (format === "quarto" || preferences.tool === "quarto") && !preferences.preset}
  <SettingRow id="build-profile" title="Quarto profile" description="Optional profile used by local Quarto builds.">
    <input class="input input-sm setting-input" aria-label="Quarto profile" value={preferences.profile || ""} onchange={(event) => chooseProfile(event.currentTarget.value)} />
  </SettingRow>
  <SettingRow id="build-parameters" title="Quarto parameters" description="JSON object passed as typed Quarto parameters.">
    <textarea class="textarea setting-input" aria-label="Quarto parameters" rows="3" value={JSON.stringify(preferences.parameters || {}, null, 2)} onchange={(event) => chooseParameters(event.currentTarget.value)}></textarea>
  </SettingRow>
  <p class="setting-description">One-shot local builds run in a temporary project copy. Live previews use the explicitly authorized project folder.</p>
{/if}
