<script>
  import Comments from "./Comments.svelte";
  let { comments = [], onhistory, ...props } = $props();
</script>

<Comments {...props} {comments} filter="suggestions" cardIdPrefix="changes" title="Changes"
  tools={[{ id: "editing", icon: "pencil", label: "Suggest a change", title: "Suggest a replacement for the selected passage" }]}
  emptyMessage="No suggested changes yet." {onhistory} />
