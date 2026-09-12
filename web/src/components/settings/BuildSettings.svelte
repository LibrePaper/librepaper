<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/latex/local.js";
  import { buildersFor, capabilityFor, supportsOperation } from "../../lib/build-catalog.js";
  import { update } from "../../lib/build-preferences.js";

  let { format = "", documentId = "", userId = "anonymous", preferences = {}, onpreferences } = $props();
  let local = $state(localBridge.status());
  $effect(() => localBridge.subscribe((status) => (local = status)));
  const builders = $derived(buildersFor(format));
  const localBuilders = $derived(builders.filter((entry) => entry.backend.includes("local")));
  const browserBuilders = $derived(builders.filter((entry) => entry.backend.includes("browser")));
  const selected = $derived(preferences.selection === "tool" ? (preferences.preset ? `local:preset:${preferences.preset}` : preferences.tool === "tex" && preferences.engine ? `${preferences.backend || "browser"}:tex:${preferences.engine}` : optionValue(preferences.backend || "browser", preferences.tool || "")) : "automatic");

  function scope() { return { origin: globalThis.location?.origin || "", user: userId, document: documentId }; }
  function choose(selection, backend, tool = "") {
    const next = update(scope(), format, {
      selection, backend, ...(tool ? { tool } : {}),
      ...(tool === "tex" || tool === "latexmk" ? { engine: "pdflatex" } : {}),
    });
    onpreferences?.(next);
  }
  function chooseEngine(engine) {
    const next = update(scope(), format, { selection: "tool", backend: preferences.backend || "browser", tool: preferences.tool || "tex", engine });
    onpreferences?.(next);
  }
  function chooseOutput(output) {
    onpreferences?.(update(scope(), format, { output }));
  }
  function chooseProfile(profile) { onpreferences?.(update(scope(), format, { profile: profile.trim() || null })); }
  function chooseParameters(text) {
    try { const parameters = text.trim() ? JSON.parse(text) : {}; if (!parameters || Array.isArray(parameters) || typeof parameters !== "object") return; onpreferences?.(update(scope(), format, { parameters })); } catch { /* leave the last valid map */ }
  }
  function disabled(entry) { const capability = capabilityFor(local?.capabilities, entry.id); const protocol = Math.max(...(local?.protocol || [1])); return entry.backend.includes("local") && (!capability || capability.available !== true || (entry.minProtocol && protocol < entry.minProtocol) || !supportsOperation(local?.capabilities, entry.id, "build", "snapshot")); }
  function disabledEngine(entry, engine) { const capability = capabilityFor(local?.capabilities, entry.id); return disabled(entry) || (Array.isArray(capability?.engines) && !capability.engines.includes(engine)); }
  function disabledOutput(output) {
    if (preferences.backend !== "local") return format === "markdown" || format === "quarto" ? output !== "html" : false;
    const capability = capabilityFor(local?.capabilities, preferences.tool);
    return !capability || !Array.isArray(capability.outputs) || !capability.outputs.includes(output);
  }
  const statusMessage = $derived(({ unknown: "Checking for the local companion…", unreachable: "Local companion unavailable.", denied: "Local network access was blocked by the browser.", reachable: "Local companion is running; connect this document.", unauthorized: "Connect this document to use local tools.", incompatible: "Update the local companion to use these tools.", connected: "Local companion connected." })[local?.state] || "");
  function version(entry) { return capabilityFor(local?.capabilities, entry.id)?.version; }
  function protocolUnavailable(entry) { return entry.minProtocol && Math.max(...(local?.protocol || [1])) < entry.minProtocol; }
  const savedMissing = $derived(preferences.selection === "tool" && preferences.tool && !builders.some((entry) => entry.id === preferences.tool) ? preferences.tool : "");
  const selectedPreset = $derived(preferences.preset ? presets.find((item) => item.id === preferences.preset) : null);
  const presetSchema = $derived(Object.entries(selectedPreset?.option_schema || {}).map(([name, schema]) => ({ name, ...schema })));
  const outputChoices = $derived.by(() => {
    const supported = preferences.backend === "local" ? capabilityFor(local?.capabilities, preferences.tool)?.outputs || [] : format === "typst" ? ["pdf", "html"] : ["html"];
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
    if (value === "automatic") return choose("automatic", "auto");
    const [backend, id, ...rest] = value.split(":");
    const preset = id === "preset" ? rest.join(":") : "";
    const engine = id === "tex" ? rest[0] : "";
    const entry = preset && presets.find((item) => item.id === preset);
    const next = update(scope(), format, { selection: "tool", backend, tool: preset ? (entry?.base_adapter || "") : id, output: ["latex", "typst"].includes(format) ? "pdf" : "html", ...(engine ? { engine } : {}), ...(preset ? { preset } : {}) });
    onpreferences?.(next);
  }
  async function rescan() { try { await localBridge.capabilities({ rescan: true }); } catch { /* status explains failure */ } }
  async function connect() { try { await localBridge.connectViaApp(); } catch { /* local status carries instructions */ } }
  const presets = $derived.by(() => {
    const found = Array.isArray(local?.capabilities?.presets) ? [...local.capabilities.presets] : [];
    const compatible = found.filter((item) => (!Array.isArray(item.source_formats) || item.source_formats.includes(format)) && (!item.base_adapter || builders.some((entry) => entry.id === item.base_adapter)));
    if (preferences.preset && !compatible.some((item) => item.id === preferences.preset)) {
      compatible.push({ id: preferences.preset, name: `${preferences.preset} (unavailable)`, available: false });
    }
    return compatible.map((preset) => ({ ...preset, available: preset.available !== false && local?.state === "connected" && local?.protocol?.includes(2) && capabilityFor(local?.capabilities, preset.base_adapter)?.available === true }));
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
        {#if entry.id === "tex"}
          <option value="browser:tex:pdflatex">pdfLaTeX</option><option value="browser:tex:xelatex">XeLaTeX</option>
        {:else}<option value={optionValue("browser", entry.id)}>{entry.label}</option>{/if}
      {/each}
    </optgroup>{/if}
    {#if localBuilders.length}<optgroup label="Local companion">
      {#each localBuilders as entry (entry.id)}
        {#if entry.id === "tex"}
          {#each ["pdflatex", "xelatex", "lualatex"] as engine}<option value={`local:tex:${engine}`} disabled={disabledEngine(entry, engine)}>{engine === "pdflatex" ? "pdfLaTeX" : engine === "xelatex" ? "XeLaTeX" : "LuaLaTeX"}{version(entry) ? ` (${version(entry)})` : ""}{protocolUnavailable(entry) ? " (Update companion)" : disabledEngine(entry, engine) ? " (unavailable)" : ""}</option>{/each}
        {:else}<option value={optionValue("local", entry.id)} disabled={disabled(entry)}>{entry.label}{version(entry) ? ` (${version(entry)})` : ""}{protocolUnavailable(entry) ? " (Update companion)" : disabled(entry) ? " (unavailable)" : ""}</option>{/if}
      {/each}
    </optgroup>{/if}
    {#if presets.length}<optgroup label="Custom presets">
      {#each presets as preset (preset.id)}<option value={`local:preset:${preset.id}`} disabled={preset.available === false}>{preset.display_name || preset.name || preset.id}</option>{/each}
    </optgroup>{/if}
    {#if savedMissing}<option value={`local:${savedMissing}`} disabled>{savedMissing} (unavailable)</option>{/if}
  </select>
</SettingRow>

<SettingRow title="Local tools" description="Refresh installed tools and companion presets.">
  <span class="setting-description" role="status">{statusMessage}</span>
  {#if ["unknown", "unreachable", "denied"].includes(local?.state)}<button type="button" class="btn btn-sm preset-filled-primary-500" onclick={connect}>Open companion</button>{/if}
  {#if ["unauthorized", "reachable"].includes(local?.state)}<button type="button" class="btn btn-sm preset-filled-primary-500" onclick={connect}>Connect</button>{/if}
  {#if local?.state !== "connected"}<a class="btn btn-sm preset-outlined-surface-300-700" href="https://github.com/LibrePaper/librepaper/releases/latest" target="_blank" rel="noreferrer">Install companion</a>{/if}
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => localBridge.probe({ force: true })}>Retry</button>
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={local?.state !== "connected"} onclick={rescan}>Rescan</button>
</SettingRow>

{#if preferences.selection === "tool" && preferences.tool === "latexmk" && !preferences.preset}
  <SettingRow id="build-engine" title="Engine" description="Choose the engine used by this build tool.">
    <select class="select setting-select" aria-label="Build engine" value={preferences.engine || "pdflatex"} onchange={(event) => chooseEngine(event.currentTarget.value)}>
      {#each (preferences.tool === "tex" && preferences.backend === "browser" ? ["pdflatex", "xelatex"] : ["pdflatex", "xelatex", "lualatex"]) as engine}
        <option value={engine} disabled={disabledEngine({ id: "latexmk", backend: ["local"], minProtocol: 2 }, engine)}>{engine === "pdflatex" ? "pdfLaTeX" : engine === "xelatex" ? "XeLaTeX" : "LuaLaTeX"}</option>
      {/each}
    </select>
  </SettingRow>
{/if}

{#if selectedPreset && presetSchema.length}
  <fieldset class="setting-group" disabled={selectedPreset.available === false}>
    <legend>Preset options</legend>
    {#each presetSchema as option (option.name)}
      <SettingRow title={option.label || option.name} description={option.description || ""}>
        {#if option.kind === "boolean"}
          <input type="checkbox" aria-label={option.label || option.name} checked={Boolean(preferences.options?.[option.name])} onchange={(event) => setPresetOption(option.name, event.currentTarget.checked)} />
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
    <select class="select setting-select" aria-label="Build output" value={preferences.output || (format === "typst" ? "pdf" : "html")} onchange={(event) => chooseOutput(event.currentTarget.value)}>
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
