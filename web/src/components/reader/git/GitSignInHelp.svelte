<script>
  // The steps to take when git could not sign in to the remote, for the host
  // and the platform it is on (lib/gitsync/sign-in-help.js). Git signs in with
  // the person's own credential helper or SSH key; LibrePaper holds neither,
  // so all it can do is say what to set up and try again.
  import IconButton from "../../IconButton.svelte";
  import { currentPlatform, signInHelp } from "../../../lib/gitsync/sign-in-help.js";

  let { remote = "", detail = "", busy = false, onretry, ondismiss } = $props();

  const help = $derived(signInHelp(remote, currentPlatform()));
  let copied = $state("");
  let copyTimer;

  async function copyCommand(command) {
    try {
      await navigator.clipboard.writeText(command);
      copied = command;
    } catch {
      // A blocked clipboard leaves the command on screen to select.
      copied = "";
      return;
    }
    clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copied = ""), 1800);
  }
  $effect(() => () => clearTimeout(copyTimer));
</script>

<div class="git-alert" role="alert">
  <p class="lp-text-error">{help.title}</p>
  {#if detail}<p class="panel-meta git-detail">{detail}</p>{/if}
  <ol class="git-steps">
    {#each help.steps as step (step.text)}
      <li>
        {#if step.href}
          <a href={step.href} target="_blank" rel="noopener noreferrer">{step.text}</a>
        {:else}
          <span>{step.text}</span>
        {/if}
        {#if step.command}
          <span class="git-command">
            <code>{step.command}</code>
            <IconButton icon={copied === step.command ? "check" : "copy"} label={`Copy ${step.command}`} tone="plain" size="btn-icon-sm"
                        onclick={() => copyCommand(step.command)} />
          </span>
        {/if}
      </li>
    {/each}
  </ol>
  <div class="git-alert-actions">
    <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => onretry?.()}>Try again</button>
    <button type="button" class="git-link-button" onclick={() => ondismiss?.()}>Dismiss</button>
    <a class="git-more" href="/docs/git.html#credentials" target="_blank" rel="noopener noreferrer">Read more</a>
  </div>
</div>

<style>
  .git-alert { margin-top: calc(var(--spacing) * 3); padding: calc(var(--spacing) * 2); border-radius: var(--radius-base); background: var(--color-row-hover); overflow-wrap: anywhere; }
  .git-alert p { margin: 0 0 calc(var(--spacing) * 1.5); }
  .git-steps { display: grid; gap: calc(var(--spacing) * 1.5); margin: 0 0 calc(var(--spacing) * 2); padding-inline-start: 1.25rem; list-style: decimal; }
  .git-steps a { color: var(--color-link); }
  .git-command { display: flex; align-items: start; gap: calc(var(--spacing) * 1); min-width: 0; margin-top: calc(var(--spacing) * .5); }
  .git-command code { flex: 1 1 auto; min-width: 0; padding: .1rem .3rem; border-radius: .25rem; background: var(--color-sidebar); font-family: var(--font-mono, monospace); overflow-wrap: anywhere; white-space: normal; }
  .git-command :global(button) { flex: none; }
  .git-alert-actions { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); }
  .git-link-button { padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .git-link-button:hover { background: var(--color-row-hover); }
  .git-more { color: var(--color-link); }
</style>
