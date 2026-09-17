// A comment and its replies as a conversation, kept apart from the card that
// draws it so the grouping rule is checkable without a browser.

/// Who said something, with the empty name written out. Nobody is called "",
/// and a card that leaves the name off has no identity in it at all.
export function named(creator) {
  return String(creator ?? "").trim() || "Anonymous";
}

/// The note and its replies, collapsed into runs of one author: two things
/// said in a row by the same person are one person talking, and the card
/// shows the avatar, the name and the time once at the top of the run rather
/// than on every line. A reply from somebody else opens the next run.
///
/// A run is keyed on the display name, because the name is all a client is
/// given. The author key behind it (`Comment::author`) is deliberately never
/// serialized: for an anonymous visitor it is a digest of their token, and
/// sending it would let one document's reader be recognized in another. Two
/// people who choose the same name therefore share a run here, and share an
/// avatar everywhere else -- the price of not shipping that key.
///
/// The note itself is always the first thing said, even when it carries no
/// words: a highlight with no note still has an author, and this is where
/// that author is written down.
export function threadRuns(comment) {
  const posts = [
    { id: `note-${comment?.id}`, creator: comment?.creator, created: comment?.created, body: comment?.body },
    ...(comment?.replies ?? []).map((reply) => ({
      id: `reply-${reply.id}`,
      creator: reply.creator,
      created: reply.created,
      body: reply.body,
    })),
  ];
  const runs = [];
  for (const post of posts) {
    const author = named(post.creator);
    const run = runs.at(-1);
    if (run && run.author === author) run.posts.push(post);
    else runs.push({ id: post.id, author, created: post.created, posts: [post] });
  }
  return runs;
}
