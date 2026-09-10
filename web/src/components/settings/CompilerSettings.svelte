<script>
  // How a LaTeX project's PDF is built: choices shared by everyone editing it.
  import SettingRow from "./SettingRow.svelte";
  import * as latex from "../../lib/latex.js";

  let { latexSettings = { engine: "auto", release: null }, onlatexsettings } = $props();

  function setEngine(engine) {
    onlatexsettings?.({ ...latexSettings, engine });
  }

  let releases = $state(null);
  $effect(() => {
    latex
      .releases()
      .then((answer) => (releases = answer))
      .catch(() => {});
  });

  let previousRelease = $state(null);
  function pinRelease(id) {
    previousRelease = latexSettings.release;
    onlatexsettings?.({ ...latexSettings, release: id });
  }

  const behind = $derived(Boolean(releases && latexSettings.release && latexSettings.release !== releases.default));
  const undoable = $derived(Boolean(previousRelease && previousRelease !== latexSettings.release));
  const version = $derived(
    !releases ? "Checking which compiler this project builds with."
    : behind ? "This project is pinned to an earlier browser compiler than the one this site offers."
    : "This project builds with the current browser compiler."
  );
</script>

<SettingRow id="compiler-engine" title="PDF compiler"
            description="Leave this on Automatic unless your template requires a particular compiler.">
  <select class="select setting-select" aria-label="Project engine" value={latexSettings.engine || "auto"}
          onchange={(event) => setEngine(event.currentTarget.value)}>
    <option value="auto">Automatic</option>
    <option value="pdflatex">pdfLaTeX</option>
    <option value="xelatex">XeLaTeX</option>
    <option value="lualatex">LuaLaTeX</option>
  </select>
</SettingRow>

<SettingRow id="compiler-release" title="Browser compiler version" description={version}>
  {#if behind}
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => pinRelease(releases.default)}>Update</button>
  {/if}
  {#if undoable}
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => pinRelease(previousRelease)}>Undo update</button>
  {/if}
</SettingRow>
