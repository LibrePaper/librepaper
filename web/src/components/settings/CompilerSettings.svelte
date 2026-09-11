<script>
  // How a LaTeX project's PDF is built: choices shared by everyone editing it.
  import SettingRow from "./SettingRow.svelte";

  let { latexSettings = { engine: "auto" }, onlatexsettings } = $props();

  function setEngine(engine) {
    onlatexsettings?.({ ...latexSettings, engine });
  }

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
