import { DurableObject } from "cloudflare:workers";

// Shared limits, injected at build time from config.go. room.js is concatenated
// ahead of worker.js into one module, so CONFIG is in scope for both.
const CONFIG = __CONFIG__;
const CAPS = CONFIG.caps;
const MOTIVATIONS = CONFIG.motivations;
const MAX_COMMENTS = CONFIG.max_comments;
const RATE_PER_HOUR = CONFIG.rate_per_hour;

const now = () => new Date().toISOString().replace(/\.\d+Z$/, "Z");
const clean = (value, limit) =>
  String(value ?? "")
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "")
    .slice(0, limit);

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
    const bucket = `rl:${ip}:${Math.floor(Date.now() / 3600000)}`;
    const count = ((await this.ctx.storage.get(bucket)) || 0) + 1;
    if (count > RATE_PER_HOUR) return false;
    await this.ctx.storage.put(bucket, count);
    // Expire the counters rather than accumulating a row per IP per hour.
    if ((await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + 3600000);
    }
    return true;
  }

  async alarm() {
    const stale = await this.ctx.storage.list({ prefix: "rl:" });
    const current = String(Math.floor(Date.now() / 3600000));
    const drop = [...stale.keys()].filter((key) => !key.endsWith(`:${current}`));
    if (drop.length) await this.ctx.storage.delete(drop);
    const example = await this.ctx.storage.get("example");
    if (example) {
      const revision = (await this.ctx.storage.get("example_revision")) || "";
      await this.resetExample(example, revision);
    }
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
        replies: (seed.replies || []).map((body) => ({ id: crypto.randomUUID(), body, creator: "Reviewer", created: stamp })),
      };
      comments.push(comment);
      await this.persist(comment);
    }
    await this.ctx.storage.put({ seq, example: slug, example_revision: revision });
    this.cache = comments;
    this.broadcast({ type: "hello", comments });
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
      // The login was verified by the Worker before this request reached here.
      server.serializeAttachment({
        ip: request.headers.get("cf-connecting-ip") || "",
        login: request.headers.get("x-komodoc-login") || "",
      });
      server.send(JSON.stringify({ type: "hello", comments: await this.load() }));
      return new Response(null, { status: 101, webSocket: client });
    }

    // REST fallback, for clients that cannot hold a socket.
    if (request.method === "GET") {
      return Response.json({ comments: await this.load() });
    }
    if (request.method === "POST") {
      const message = await request.json();
      const result = await this.apply(
        message,
        request.headers.get("cf-connecting-ip") || "",
        request.headers.get("x-komodoc-login") || "",
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
    const { ip, login } = socket.deserializeAttachment() || {};
    const result = await this.apply(message, ip, login);
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
  async apply(message, ip, login) {
    const fail = (text) => ({ type: "error", message: text, temp_id: message.temp_id });
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

    if (message.type === "resolve") {
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
      const index = comments.findIndex((item) => item.id === message.comment_id);
      if (index < 0) return fail("unknown comment");
      const [comment] = comments.splice(index, 1);
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

    // Labels: lowercased, trimmed, deduplicated, capped, so filtering by one
    // of them is predictable.
    const tags = [];
    for (const raw of Array.isArray(message.tags) ? message.tags : []) {
      const label = clean(raw, CAPS.tag).trim().toLowerCase().replace(/\s+/g, " ");
      if (label && !tags.includes(label)) tags.push(label);
      if (tags.length === CONFIG.max_tags) break;
    }

    if (message.type === "reply") {
      const comment = comments.find((item) => item.id === message.comment_id);
      if (!comment) return fail("unknown comment");
      const reply = { id: crypto.randomUUID(), body, creator, created: now() };
      comment.replies.push(reply);
      await this.persist(comment);
      return { type: "reply", comment_id: comment.id, reply, temp_id: message.temp_id };
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
      };
      comments.push(comment);
      await this.persist(comment);
      return { type: "comment", comment, temp_id: message.temp_id };
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
