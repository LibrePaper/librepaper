<script>
  // How the whole app looks, what the desktop app does when its window closes, and the diagnostics report.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { choice, onChoice, setChoice, PALETTES, PALETTE_NAMES, palette, onPalette, setPalette, PAPERS, paper, onPaper, setPaper } from "../../lib/theme.js";
  import DiagnosticsPanel from "./DiagnosticsPanel.svelte";
  import { desktop, shellAction } from "../../lib/desktop/client.js";
  import { LANGUAGES, chosenLanguage, chooseLanguage, locale } from "../../lib/locale.js";
  import { ISSUES_URL } from "../../lib/site-links.js";
  import { m } from "../../lib/paraglide/messages.js";

  const language = chosenLanguage();

  // A wrong or awkward translation goes to the issue tracker as a new issue,
  // already titled with the language it is in. The questions stay in English,
  // because whoever fixes the catalogue reads them.
  const translationIssue = (() => {
    const tag = locale();
    const body = `Language: ${tag}\n\nWhere it appears:\n\nWhat it says now:\n\nWhat it should say:\n`;
    return `${ISSUES_URL}?title=${encodeURIComponent(`Translation (${tag}): `)}&body=${encodeURIComponent(body)}`;
  })();

  // Follows the choice, which another tab of this browser may make too.
  let current = $state(choice());
  onMount(() => onChoice((now) => { current = now; }));

  let currentPalette = $state(palette());
  onMount(() => onPalette((now) => { currentPalette = now; }));

  let currentPaper = $state(paper());
  onMount(() => onPaper((now) => { currentPaper = now; }));

  // What closing the last window does in the desktop app. The shell saves it.
  let closing = $state(desktop()?.close === "quit" ? "quit" : "tray");
  let closeError = $state("");
  async function chooseClose(next) {
    const before = closing;
    closing = next;
    closeError = "";
    try {
      await shellAction("close", { close: next });
    } catch (cause) {
      closing = before;
      closeError = cause?.message || m.generalsettings_close_failed();
    }
  }
</script>

<SettingRow id="general-language" title={m.generalsettings_language()}>
  <select class="select setting-select" aria-label={m.generalsettings_language()} value={language}
          onchange={(event) => chooseLanguage(event.currentTarget.value)}>
    <option value="system">{m.generalsettings_language_system()}</option>
    {#each Object.entries(LANGUAGES) as [code, name] (code)}
      <option value={code} lang={code}>{name}</option>
    {/each}
  </select>
  {#snippet note()}<a class="translation-report" href={translationIssue} target="_blank" rel="noreferrer">{m.generalsettings_report_translation()}</a>{/snippet}
</SettingRow>

<SettingRow id="appearance-theme" title={m.generalsettings_theme()}>
  <select class="select setting-select" aria-label={m.generalsettings_theme()} value={current}
          onchange={(event) => setChoice(event.currentTarget.value)}>
    <option value="system">{m.generalsettings_theme_system()}</option>
    <option value="light">{m.generalsettings_theme_light()}</option>
    <option value="dark">{m.generalsettings_theme_dark()}</option>
  </select>
</SettingRow>

<SettingRow id="appearance-colours" title={m.generalsettings_colours()}>
  <select class="select setting-select" aria-label={m.generalsettings_colours()} value={currentPalette}
          onchange={(event) => setPalette(event.currentTarget.value)}>
    {#each PALETTES as name (name)}
      <option value={name}>{PALETTE_NAMES[name]}</option>
    {/each}
  </select>
</SettingRow>

<SettingRow id="appearance-preview-paper" title={m.generalsettings_preview_paper()}
            description={m.generalsettings_preview_paper_description()}>
  <select class="select setting-select" aria-label={m.generalsettings_preview_paper()} value={currentPaper}
          onchange={(event) => setPaper(event.currentTarget.value)}>
    {#each PAPERS as name (name)}
      <option value={name}>{name === "theme" ? m.generalsettings_paper_theme() : m.generalsettings_paper_light()}</option>
    {/each}
  </select>
</SettingRow>

{#if desktop()}
  <SettingRow id="general-close" title={m.generalsettings_close_title()}
              description={m.generalsettings_close_description()}>
    <select class="select setting-select" aria-label={m.generalsettings_close_title()} value={closing}
            onchange={(event) => chooseClose(event.currentTarget.value)}>
      <option value="tray">{m.generalsettings_close_tray()}</option>
      <option value="quit">{m.generalsettings_close_quit()}</option>
    </select>
    {#snippet note()}{#if closeError}<span class="close-error" role="alert">{closeError}</span>{/if}{/snippet}
  </SettingRow>
{/if}

<section id="diagnostics-report" class="settings-subsection"><DiagnosticsPanel /></section>

<style>
  .translation-report {
    color: var(--color-brand);
    font-size: var(--panel-meta-size);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .close-error {
    color: var(--panel-muted);
    font-size: var(--panel-meta-size);
  }
</style>
