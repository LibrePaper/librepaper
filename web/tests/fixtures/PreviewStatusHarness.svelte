<script>
  import PreviewStatus from "../../src/components/PreviewStatus.svelte";
</script>

<div>
  <PreviewStatus label="Preview needs attention" tone="error">
    {#snippet details(close)}
      <button class="harness-action" onclick={() => close()}>Act</button>
    {/snippet}
  </PreviewStatus>
  <iframe class="harness-frame" srcdoc="<p>frame</p>"></iframe>
</div>
