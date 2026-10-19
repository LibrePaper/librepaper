<script>
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import Chat from "../Chat.svelte";
  import Comments from "../Comments.svelte";
  import PanelTabs from "../PanelTabs.svelte";

  let {
    messages = [], connected = false, canPost = false, onsend = undefined,
    more = false, loadingOlder = false, loadError = "", canClear = false, onloadolder = undefined, onclear = undefined,
    comments = [], identity = "", commentingAs = "Anonymous",
    canModerate = false, canComment = true,
    onreveal = undefined, onresolve = undefined, ondelete = undefined,
    ondeletemany = undefined, onreply = undefined, onedit = undefined, onagent = undefined, pending = undefined,
    unreadChat = false, selected = "", tab = $bindable("comments"),
    // The traversal, as `lib/reader/annotations.svelte.js` keeps it: the
    // catalogue's own counts, whether there is another page, and whether
    // the last request for one failed. The panel shows these rather than
    // counting the rows it was handed, which are a prefix.
    page = null, onloadmore, onloadreplies,
    // Editors only: attaching a selection to a comment, and its moment.
    onattach = undefined, attachable = false, onmoment = undefined,
    // The draft being written, which belongs to the Comments tab whatever it
    // will become.
    composing = null, needsLogin = false, signInHref = "", oncommentsend = undefined, oncommentcancel = undefined,
  } = $props();
  // View-only links never see the chat, so there is no tab for it.
  const VIEWS = $derived([
    ...(canComment ? [{ id: "chat", label: "Chat" }] : []),
    { id: "comments", label: "Comments" },
    { id: "highlights", label: "Highlights" },
  ]);
  $effect(() => { if (!canComment && tab === "chat") tab = "comments"; });
  const common = () => ({ identity, commentingAs, canModerate, canComment,
    onreveal, onresolve, ondelete, ondeletemany,
    onreply, onedit, onagent, pending, selected,
    page, onloadmore, onloadreplies, onattach, attachable, onmoment });
</script>

<!-- The strip, its ids and the layout of a pane are PanelTabs', shared with
     the agent panel. What is left here is what the tabs contain. -->
<section class="collaboration panel panel-tabbed" aria-label="Collaboration">
  <PanelTabs id="collaboration" label="Collaboration views" listClass="collab-tabs"
             tabs={VIEWS.map((view) => ({ ...view,
               dot: view.id === "chat" && unreadChat ? "Unread messages" : "" }))}
             value={tab} onchange={(value) => (tab = value)}>
    <Tabs.Content value="chat"><Chat {messages} {more} {loadingOlder} {loadError} {canClear} {onloadolder} {onclear} {connected} {canPost} {onsend} /></Tabs.Content>
    <Tabs.Content value="comments">
      <Comments {...common()} {comments} filter="comments" cardIdPrefix="collaboration-comment"
        {composing} {needsLogin} {signInHref} onsend={oncommentsend} oncancel={oncommentcancel} />
    </Tabs.Content>
    <Tabs.Content value="highlights">
      <Comments {...common()} {comments} filter="highlights" title="Highlights"
        cardIdPrefix="collaboration-highlight"
        hint="Select text in the preview window to highlight." />
    </Tabs.Content>
  </PanelTabs>
</section>
