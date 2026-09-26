<script>
  import Nav from "./Nav.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
</script>

<Nav />
<Page>
  <Stack gap={4}>
    <h1 class="h2">Nothing here</h1>
    <p class="lp-text-secondary">
      This link does not point at a document on this deployment. It may have been deleted, it may
      have expired, or the address may have been mistyped.
    </p>
    <a class="anchor" href="/">Go to the document list</a>
  </Stack>
</Page>
