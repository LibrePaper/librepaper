import { DurableObject } from "cloudflare:workers";

// Shared limits, injected at build time from config.go. room.js is concatenated
// ahead of worker.js into one module, so CONFIG is in scope for both.
const CONFIG = __CONFIG__;
const CAPS = CONFIG.caps;
const MOTIVATIONS = CONFIG.motivations;
const MAX_COMMENTS = CONFIG.max_comments;
const RATE_PER_HOUR = CONFIG.rate_per_hour;
// A comment thread that could grow forever would make one comment's replies
// as expensive to load as the whole document's comments; config.go is being
// extended with this limit concurrently, so fall back to its default here
// until that lands.
const MAX_REPLIES = CONFIG.max_replies ?? 100;

const now = () => new Date().toISOString().replace(/\.\d+Z$/, "Z");

// rateKey collapses an IPv6 address to its /64 -- the block an ISP hands one
// customer -- so a rate limit keyed on it bounds one customer, not one of the
// many addresses inside their assigned block. IPv4 addresses are used whole.
// worker.js reuses this for the example-room identity, since a room per
// address should mean one room per customer too.
function rateKey(address) {
  const value = String(address || "");
  if (!value.includes(":")) return value;
  let hextets = value.split(":");
  if (value.includes("::")) {
    const [head, tail] = value.split("::");
    const headParts = head ? head.split(":") : [];
    const tailParts = tail ? tail.split(":") : [];
    const missing = 8 - headParts.length - tailParts.length;
    hextets = [...headParts, ...Array(Math.max(missing, 0)).fill("0"), ...tailParts];
  }
  return hextets.slice(0, 4).join(":");
}

// Labels: lowercased, trimmed, deduplicated, capped in both length and number,
// so filtering by one of them is predictable. cleanTags in room.go has to agree
// with this one, and conformance_test.go runs both over the same fixtures.
function cleanTags(list) {
  const tags = [];
  for (const raw of Array.isArray(list) ? list : []) {
    const label = clean(raw, CAPS.tag).trim().toLowerCase().replace(/\s+/g, " ");
    if (label && !tags.includes(label)) tags.push(label);
    if (tags.length === CONFIG.max_tags) break;
  }
  return tags;
}
const clean = (value, limit) =>
  String(value ?? "")
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "")
    .slice(0, limit);

// authorKeyFor is who a comment belongs to: a signed-in caller by their login,
// an anonymous one by the visitor cookie the Worker verified for them, nobody
// otherwise. worker.js computes login and visitor the same way and passes them
// in as headers, so this is the only place that turns them into the key
// stored on a comment and compared against on delete.
function authorKeyFor(login, visitor) {
  if (login) return `github:${login.toLowerCase()}`;
  if (visitor) return `visitor:${visitor}`;
  return "";
}

// identityFromHeaders reads the three headers the Worker sets on every
// request it forwards here -- login, visitor hash, and whether the caller
// owns the document -- and never anything a client could have set itself,
// since the Worker always overwrites them before the request reaches a Room.
function identityFromHeaders(request) {
  return {
    login: request.headers.get("x-komodoc-login") || "",
    visitor: request.headers.get("x-komodoc-visitor") || "",
    owner: request.headers.get("x-komodoc-owner") === "1",
  };
}

// forClient is what one caller may see of one comment: never the author key
// that identifies who wrote it, but a deletable flag computed just for them.
// Used for the hello snapshot and the GET fallback, both single-caller views;
// broadcasts use stripAuthor below instead, since they carry no such flag.
function forClient(comment, authorKey, owner) {
  const { author, replies, ...rest } = comment;
  return {
    ...rest,
    deletable: Boolean(owner) || (Boolean(authorKey) && author === authorKey),
    replies: replies.map(({ author: _replyAuthor, ...reply }) => reply),
  };
}

// stripAuthor removes the same field for a broadcast, which goes to every
// open socket at once and so carries no per-caller deletable flag either.
function stripAuthor(comment) {
  const { author, replies, ...rest } = comment;
  return { ...rest, replies: replies.map(({ author: _replyAuthor, ...reply }) => reply) };
}

/**
 * One instance per document slug. It owns that document's comments and holds
 * the open sockets of everyone currently reading it, so a write by one reader
 * reaches the others without anybody polling.
 */
export class Room extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.cache = null;
  }

  // Comment volume per document is in the dozens; keeping the whole list in
  // memory keeps the broadcast path free of storage reads.
  async load() {
    if (this.cache) return this.cache;
    const stored = await this.ctx.storage.list({ prefix: "c:" });
    this.cache = [...stored.values()].sort((a, b) => a.seq - b.seq);
    return this.cache;
  }

  async nextSeq() {
    const seq = ((await this.ctx.storage.get("seq")) || 0) + 1;
    await this.ctx.storage.put("seq", seq);
    return seq;
  }

  async persist(comment) {
    await this.ctx.storage.put(`c:${String(comment.seq).padStart(6, "0")}`, comment);
  }

  broadcast(payload) {
    const message = JSON.stringify(payload);
    for (const socket of this.ctx.getWebSockets()) {
      try {
        socket.send(message);
      } catch {
        /* the socket is going away; close handling cleans it up */
      }
    }
  }

  async rateOk(ip) {
    if (!ip) return true;
    const bucket = `rl:${rateKey(ip)}:${Math.floor(Date.now() / 3600000)}`;
    const count = ((await this.ctx.storage.get(bucket)) || 0) + 1;
    if (count > RATE_PER_HOUR) return false;
    await this.ctx.storage.put(bucket, count);
    // Expire the counters rather than accumulating a row per IP per hour.
    if ((await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + 3600000);
    }
    return true;
  }

  // An example room is keyed by document and visitor (documentRoom in
  // worker.js), so a client that rotates its visitor cookie can otherwise
  // manufacture an unbounded number of these objects, each seeded from R2 and
  // never freed. The alarm every /ensure arms is what bounds that: a room
  // still being read gets its sandbox reset, as before, while one nobody has
  // open is emptied out, leaving nothing behind until the next visit reseeds
  // it.
  async alarm() {
    const stale = await this.ctx.storage.list({ prefix: "rl:" });
    const current = String(Math.floor(Date.now() / 3600000));
    const drop = [...stale.keys()].filter((key) => !key.endsWith(`:${current}`));
    if (drop.length) await this.ctx.storage.delete(drop);
    const example = await this.ctx.storage.get("example");
    if (!example) return;
    if (this.ctx.getWebSockets().length === 0) {
      await this.ctx.storage.deleteAll();
      this.cache = null;
      return;
    }
    const revision = (await this.ctx.storage.get("example_revision")) || "";
    await this.resetExample(example, revision);
    await this.ctx.storage.setAlarm(Date.now() + 3600000);
  }

  async resetExample(slug, revision = "") {
    const stored = await this.env.DOCS.get(`examples/${slug}.json`);
    if (!stored) return;
    const seeds = await stored.json();
    const old = await this.ctx.storage.list({ prefix: "c:" });
    if (old.size) await this.ctx.storage.delete([...old.keys()]);
    let seq = 0;
    const stamp = now();
    const comments = [];
    for (const seed of seeds) {
      const comment = {
        id: crypto.randomUUID(), seq: ++seq,
        motivation: seed.motivation || "commenting",
        exact: seed.exact || "", prefix: seed.prefix || "", suffix: seed.suffix || "",
        position: Number.isInteger(seed.position) ? seed.position : null,
        region: seed.region || null, body: seed.body || "", replacement: seed.replacement || "",
        tags: seed.tags || [], creator: seed.creator || "Example", created: stamp,
        resolved: Boolean(seed.resolved), resolved_at: seed.resolved ? stamp : null,
        replies: (seed.replies || []).map((body) => (
          { id: crypto.randomUUID(), body, creator: "Reviewer", created: stamp, author: "" }
        )),
        // Seeded comments belong to nobody in particular; only the document's
        // owner may clear them out.
        author: "",
      };
      comments.push(comment);
      await this.persist(comment);
    }
    await this.ctx.storage.put({ seq, example: slug, example_revision: revision });
    this.cache = comments;
    // An example room is scoped to one document and one identity (see
    // documentRoom in worker.js), so every socket attached here was opened by
    // the same caller and any one of their attachments describes them all.
    const [firstSocket] = this.ctx.getWebSockets();
    const { login = "", visitor = "", owner = false } = firstSocket ? (firstSocket.deserializeAttachment() || {}) : {};
    const authorKey = authorKeyFor(login, visitor);
    this.broadcast({ type: "hello", comments: comments.map((comment) => forClient(comment, authorKey, owner)) });
  }

  async fetch(request) {
    const url = new URL(request.url);

    if (url.pathname === "/ensure") {
      const slug = url.searchParams.get("slug") || "";
      const revision = url.searchParams.get("revision") || "";
      const current = await this.ctx.storage.get("example_revision");
      if (!(await this.ctx.storage.get("example")) || current !== revision) {
        await this.resetExample(slug, revision);
      }
      // Every example room gets a visit from alarm(), whether or not anyone
      // ever writes to it, so a room nobody reopens is eventually freed
      // rather than sitting seeded forever.
      if ((await this.ctx.storage.getAlarm()) === null) {
        await this.ctx.storage.setAlarm(Date.now() + 3600000);
      }
      return Response.json({ ready: true });
    }

    // Reached only through the delete route, which checks the caller first.
    // Wipes this document's comments and drops anyone still reading it.
    if (url.pathname === "/purge") {
      await this.ctx.storage.deleteAll();
      this.cache = null;
      for (const socket of this.ctx.getWebSockets()) {
        try {
          socket.close(1000, "document deleted");
        } catch {
          /* already going away */
        }
      }
      return Response.json({ purged: true });
    }

    if (url.pathname === "/counts") {
      const comments = await this.load();
      return Response.json({
        comment_count: comments.length,
        open_count: comments.filter((comment) => !comment.resolved).length,
      });
    }

    if (request.headers.get("upgrade") === "websocket") {
      const [client, server] = Object.values(new WebSocketPair());
      // Hibernatable: an idle document with open tabs costs nothing.
      this.ctx.acceptWebSocket(server);
      // Identity was verified by the Worker before this request reached here;
      // the attachment survives hibernation, so it travels with the socket.
      const { login, visitor, owner } = identityFromHeaders(request);
      const ip = request.headers.get("cf-connecting-ip") || "";
      server.serializeAttachment({ ip, login, visitor, owner });
      const authorKey = authorKeyFor(login, visitor);
      const comments = (await this.load()).map((comment) => forClient(comment, authorKey, owner));
      server.send(JSON.stringify({ type: "hello", comments }));
      return new Response(null, { status: 101, webSocket: client });
    }

    // REST fallback, for clients that cannot hold a socket.
    if (request.method === "GET") {
      const { login, visitor, owner } = identityFromHeaders(request);
      const authorKey = authorKeyFor(login, visitor);
      const comments = (await this.load()).map((comment) => forClient(comment, authorKey, owner));
      return Response.json({ comments });
    }
    if (request.method === "POST") {
      const message = await request.json();
      const { login, visitor, owner } = identityFromHeaders(request);
      const result = await this.apply(
        message,
        request.headers.get("cf-connecting-ip") || "",
        login,
        visitor,
        owner,
      );
      if (result.type !== "error") this.broadcast(result);
      if (result.type !== "error" && await this.ctx.storage.get("example")) {
        if ((await this.ctx.storage.getAlarm()) === null) {
          await this.ctx.storage.setAlarm(Date.now() + 3600000);
        }
      }
      return Response.json(result, { status: result.type === "error" ? 400 : 200 });
    }
    return new Response("method not allowed", { status: 405 });
  }

  async webSocketMessage(socket, raw) {
    let message;
    try {
      message = JSON.parse(raw);
    } catch {
      return;
    }
    const { ip, login, visitor, owner } = socket.deserializeAttachment() || {};
    const result = await this.apply(message, ip, login, visitor, owner);
    if (result.type === "error") {
      socket.send(JSON.stringify(result));
      return;
    }
    this.broadcast(result);
    if (await this.ctx.storage.get("example") && (await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + 3600000);
    }
  }

  async webSocketClose(socket, code, reason) {
    socket.close(code === 1006 ? 1000 : code, reason);
  }

  /** Validate, persist, and return the event to broadcast. */
  async apply(message, ip, login, visitor, owner) {
    // resolve and delete name the comment they act on, so an error about
    // either carries comment_id too -- the reader needs it to roll back the
    // optimistic update it already applied.
    const fail = (text) => ({
      type: "error",
      message: text,
      temp_id: message.temp_id,
      ...(message.type === "resolve" || message.type === "delete" ? { comment_id: message.comment_id } : {}),
    });
    const comments = await this.load();

    // Who may comment is set at deploy time, like who may publish. When it is
    // not open to anyone, the name on a comment is the verified login rather
    // than whatever the client typed. parsePolicy comes from worker.js, which
    // shares this module.
    const commenters = parsePolicy(this.env.KOMODOC_COMMENTERS);
    if (!policyAllows(commenters, login)) {
      return fail(
        login
          ? `@${login} may not comment here; this deployment allows ${describePolicy(commenters)}`
          : "sign in with GitHub to comment",
      );
    }
    // A signed-in commenter is named by their account, whether or not signing
    // in was required. Only anonymous readers type a name.
    if (login) message = { ...message, creator: login };

    // Who this comment (or reply) is attributed to, for storage only: never
    // sent back to a client, only compared against on delete.
    const authorKey = authorKeyFor(login, visitor);

    if (message.type === "resolve") {
      // Resolving is as cheap to spam as commenting, so it counts the same.
      if (!(await this.rateOk(ip))) return fail("too many comments from this address; try later");
      const comment = comments.find((item) => item.id === message.comment_id);
      if (!comment) return fail("unknown comment");
      comment.resolved = Boolean(message.resolved);
      comment.resolved_at = comment.resolved ? now() : null;
      await this.persist(comment);
      return {
        type: "resolve",
        comment_id: comment.id,
        resolved: comment.resolved,
        resolved_at: comment.resolved_at,
      };
    }

    if (message.type === "delete") {
      if (!(await this.rateOk(ip))) return fail("too many comments from this address; try later");
      const index = comments.findIndex((item) => item.id === message.comment_id);
      if (index < 0) return fail("unknown comment");
      const comment = comments[index];
      // Deleting someone else's comment takes owning the document; deleting
      // your own takes only having written it.
      const isAuthor = Boolean(authorKey) && comment.author === authorKey;
      if (!isAuthor && !owner) return fail("you may only delete your own comments");
      comments.splice(index, 1);
      await this.ctx.storage.delete(`c:${String(comment.seq).padStart(6, "0")}`);
      return { type: "delete", comment_id: comment.id };
    }

    if (!(await this.rateOk(ip))) return fail("too many comments from this address; try later");

    const body = clean(message.body, CAPS.body).trim();
    const motivation = MOTIVATIONS.includes(message.motivation)
      ? message.motivation
      : CONFIG.default_motivation;
    // A highlight is the passage itself: marking something as worth returning
    // to needs no words. Everything else is a remark, and a remark with no
    // words is nothing.
    if (!body && !(message.type === "comment" && motivation === "highlighting")) {
      return fail("comment body is required");
    }
    const creator = clean(message.creator, CAPS.creator).trim() || "Anonymous";

    // Only a suggested edit proposes replacement text; anything else sending
    // it is ignored rather than refused.
    const replacement =
      motivation === "editing" ? clean(message.replacement, CAPS.replacement).trim() : "";

    const tags = cleanTags(message.tags);

    if (message.type === "reply") {
      const comment = comments.find((item) => item.id === message.comment_id);
      if (!comment) return fail("unknown comment");
      if (comment.replies.length >= MAX_REPLIES) {
        return fail("this comment has reached its reply limit");
      }
      const reply = { id: crypto.randomUUID(), body, creator, created: now(), author: authorKey };
      comment.replies.push(reply);
      await this.persist(comment);
      const { author: _replyAuthor, ...publicReply } = reply;
      return { type: "reply", comment_id: comment.id, reply: publicReply, temp_id: message.temp_id };
    }

    if (message.type === "comment") {
      if (comments.length >= MAX_COMMENTS) {
        return fail("this document has reached its comment limit");
      }
      const exact = clean(message.exact, CAPS.exact).trim();
      const spot = validRegion(message.region);
      // An annotation is anchored to words or to part of a figure; one or the
      // other, never neither.
      if (!exact && !spot) return fail("select some text or part of a figure to comment on");
      const comment = {
        id: crypto.randomUUID(),
        seq: await this.nextSeq(),
        motivation,
        // exact, prefix and suffix are a W3C TextQuoteSelector, and are the
        // durable anchor. Offsets are recomputed in the reader against whatever
        // version of the document is on screen, so replacing a document needs
        // no migration pass here.
        exact,
        prefix: clean(message.prefix, CAPS.context),
        suffix: clean(message.suffix, CAPS.context),
        // Where the passage sat when the comment was made. Only a tie-breaker
        // for documents that repeat themselves; null on older comments.
        position: Number.isInteger(message.position) && message.position >= 0 ? message.position : null,
        region: spot,
        body,
        replacement,
        tags,
        creator,
        created: now(),
        resolved: false,
        resolved_at: null,
        replies: [],
        author: authorKey,
      };
      comments.push(comment);
      await this.persist(comment);
      return { type: "comment", comment: stripAuthor(comment), temp_id: message.temp_id };
    }

    return fail("unknown message type");
  }
}

// A rectangle on an image, in percentages of the image's own size, kept only
// if it is one: inside the image, and big enough to be worth drawing.
function validRegion(spot) {
  if (!spot || typeof spot !== "object") return null;
  const { x, y, w, h } = spot;
  const inside = (v) => typeof v === "number" && Number.isFinite(v) && v >= 0 && v <= 100;
  if (!inside(x) || !inside(y) || !inside(w) || !inside(h)) return null;
  if (w < 0.5 || h < 0.5 || x + w > 100.5 || y + h > 100.5) return null;
  const index = Number.isInteger(spot.image_index) && spot.image_index >= 0 ? spot.image_index : null;
  if (index === null) return null;
  return {
    image_digest: clean(spot.image_digest, 64),
    image_index: index,
    x,
    y,
    w,
    h,
  };
}
