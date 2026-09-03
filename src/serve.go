package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"
	"unicode/utf8"
)

// serve runs the whole service in this process: the same routes the Worker
// answers, the same socket protocol, documents on disk instead of R2, and a
// room per document instead of a Durable Object.

var (
	reSlugPath     = regexp.MustCompile(`^/ws/([^/]+)$`)
	reRawVersion   = regexp.MustCompile(`^/raw/([^/]+)/([0-9a-f]{64})\.html$`)
	reRawCurrent   = regexp.MustCompile(`^/raw/([^/]+)$`)
	reDeletePath   = regexp.MustCompile(`^/api/documents/([^/]+)/delete$`)
	reCommentsPath = regexp.MustCompile(`^/api/documents/([^/]+)/comments$`)
	reDocumentPath = regexp.MustCompile(`^/api/documents/([^/]+)$`)
	reDocsPage     = regexp.MustCompile(`^/docs/[^/]+$`)
	reSHA          = regexp.MustCompile(`^[0-9a-f]{64}$`)
)

var reSlug = regexp.MustCompile(config.SlugPattern)

const (
	// How much of a multipart upload is held in memory before the rest spills
	// to a temporary file, and how much room the whole request gets beyond the
	// document itself for part headers, the title, and the slug.
	multipartMemory = 8 << 20
	multipartSlack  = 1 << 20
)

type server struct {
	store  *store
	rooms  *roomSet
	shell  map[string]shellFile
	app    githubApp
	key    []byte
	tokens *tokenCache

	publishers policy
	commenters policy
}

// With no --port, serve takes the first free port in this range, so a second
// deployment on the same machine, or a port something else has already taken,
// needs no thought.
const (
	portFirst = 8080
	portLast  = 8099
)

// listen claims a port: the one asked for, or the first free one in the
// default range when port is zero.
func listen(port int) net.Listener {
	if port != 0 {
		listener, err := net.Listen("tcp", fmt.Sprintf(":%d", port))
		if err == nil {
			return listener
		}
		if errors.Is(err, syscall.EADDRINUSE) {
			die("port %d is already in use. Pick another with --port.", port)
		}
		die("could not listen on port %d: %v", port, err)
	}

	for candidate := portFirst; candidate <= portLast; candidate++ {
		listener, err := net.Listen("tcp", fmt.Sprintf(":%d", candidate))
		if err == nil {
			return listener
		}
		if !errors.Is(err, syscall.EADDRINUSE) {
			die("could not listen on port %d: %v", candidate, err)
		}
	}
	die("ports %d to %d are all in use. Pick one with --port.", portFirst, portLast)
	return nil
}

type serveOptions struct {
	port         int
	dir          string
	clientID     string
	clientSecret string
	publishers   string
	commenters   string
	expireAfter  string
	expireFrom   string
}

func serve(options serveOptions) {
	retention, err := parseRetention(firstOf(options.expireAfter, os.Getenv("KOMODOC_EXPIRE_AFTER")))
	if err != nil {
		die("%v; use a duration such as 24h or 30d", err)
	}
	expireFrom, err := parseExpireFrom(firstOf(options.expireFrom, os.Getenv("KOMODOC_EXPIRE_FROM")))
	if err != nil {
		die("%v", err)
	}
	dir := options.dir
	if dir == "" {
		dir = "komodoc-data"
	}
	absolute, err := filepath.Abs(dir)
	if err != nil {
		die("bad --data directory: %v", err)
	}
	if err := os.MkdirAll(filepath.Join(absolute, "comments"), 0o755); err != nil {
		die("could not create %s: %v", absolute, err)
	}

	// Claim the port first, so a port already in use costs nothing and the
	// advice below can name the callback URL this run would actually use.
	listener := listen(options.port)
	address := fmt.Sprintf(":%d", listener.Addr().(*net.TCPAddr).Port)

	app := githubApp{
		ClientID:     firstOf(options.clientID, os.Getenv("KOMODOC_GITHUB_CLIENT_ID")),
		ClientSecret: firstOf(options.clientSecret, os.Getenv("KOMODOC_GITHUB_CLIENT_SECRET")),
	}
	publishers := parsePolicy(firstOf(options.publishers, os.Getenv("KOMODOC_PUBLISHERS")))
	if len(publishers.Logins) == 0 && !publishers.Any && !publishers.Public {
		die("say who may publish, with --publishers.\n\n" +
			"    --publishers your-github-login      only you\n" +
			"    --publishers alice,bob              those accounts\n" +
			"    --publishers any                    any GitHub account\n" +
			"    --publishers anyone                 no sign-in at all")
	}
	commenters := parsePolicy(firstOf(options.commenters, os.Getenv("KOMODOC_COMMENTERS"), "anyone"))

	// The OAuth app is only needed when something here asks for a GitHub
	// account; a wholly public server runs without one.
	if !app.configured() && !(publishers.Public && commenters.Public) {
		die("this needs a GitHub OAuth app.\n\n"+
			"  Create one at https://github.com/settings/developers (New OAuth App):\n\n"+
			"    Homepage URL          http://localhost%s\n"+
			"    Authorization callback  http://localhost%s/auth/callback\n\n"+
			"  Then generate a client secret and:\n\n"+
			"    export KOMODOC_GITHUB_CLIENT_ID=...\n"+
			"    export KOMODOC_GITHUB_CLIENT_SECRET=...\n\n"+
			"  The callback has to match the port, so pass --port %s to keep it fixed.",
			address, address, strings.TrimPrefix(address, ":"))
	}

	instance := &server{
		store:      newStore(absolute),
		rooms:      newRoomSet(filepath.Join(absolute, "comments")),
		shell:      loadShell(),
		app:        app,
		key:        sessionKey(absolute),
		tokens:     newTokenCache(),
		publishers: publishers,
		commenters: commenters,
	}

	fmt.Printf("komodoc serving http://localhost%s\n", address)
	fmt.Printf("  documents on http://%s%s\n", docsPrefix+"localhost", address)
	fmt.Printf("  data in %s\n", absolute)
	fmt.Printf("  publishing: %s\n", publishers.describe())
	fmt.Printf("  commenting: %s\n", commenters.describe())
	if retention > 0 {
		fmt.Printf("  expiry: %s after %s\n", expireFrom, retention)
		instance.deleteExpired(time.Now(), retention, expireFrom)
		go instance.runJanitor(retention, expireFrom)
	}

	httpServer := &http.Server{
		Handler:           instance,
		ReadHeaderTimeout: 20 * time.Second,
	}
	if err := httpServer.Serve(listener); err != nil {
		die("%v", err)
	}
}

func (s *server) runJanitor(retention time.Duration, from string) {
	ticker := time.NewTicker(time.Hour)
	defer ticker.Stop()
	for now := range ticker.C {
		s.deleteExpired(now, retention, from)
	}
}

func (s *server) deleteDocument(slug string) (int, error) {
	s.rooms.purge(slug)
	return s.store.remove(slug)
}

func (s *server) deleteExpired(now time.Time, retention time.Duration, from string) int {
	removed := 0
	cutoff := now.Add(-retention)
	for _, entry := range s.store.list() {
		stamp, err := entry.expiryTime(from)
		if err == nil && !stamp.After(cutoff) {
			// The janitor runs unattended: a document whose index entry could
			// not be rewritten is left for the next hourly pass to retry.
			if _, err := s.deleteDocument(entry.Slug); err != nil {
				fmt.Fprintf(os.Stderr, "could not expire %s: %v\n", entry.Slug, err)
				continue
			}
			removed++
		}
	}
	return removed
}

func (s *server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	path := r.URL.Path

	// --- the document origin ------------------------------------------------
	// Requests arriving on docs.<host> get documents and the in-frame agent,
	// and nothing else: no shell, no API, no session. That is the whole point
	// of the separate hostname.
	if isDocsHost(r) {
		if match := reRawVersion.FindStringSubmatch(path); match != nil {
			s.serveDocument(w, r, match[1], match[2])
			return
		}
		if path == "/agent.js" {
			asset := s.shell["/agent.js"]
			w.Header().Set("content-type", asset.Type)
			privacyHeaders(w.Header())
			w.Header().Set("cache-control", "public, max-age=300")
			_, _ = io.WriteString(w, asset.Body)
			return
		}
		http.Error(w, "not found", http.StatusNotFound)
		return
	}

	// A document asked for on the reader's own host is sent to the other one,
	// so it is never served somewhere it could reach the session.
	if reRawVersion.MatchString(path) {
		http.Redirect(w, r, docsOrigin(r)+path, http.StatusFound)
		return
	}

	// --- signing in ---------------------------------------------------------
	if strings.HasPrefix(path, "/auth/") || path == "/api/me" || path == "/api/auth/config" {
		if s.handleAuth(w, r) {
			return
		}
	}

	// --- live comment channel ---------------------------------------------
	if match := reSlugPath.FindStringSubmatch(path); match != nil {
		if !reSlug.MatchString(match[1]) {
			http.Error(w, "bad slug", http.StatusBadRequest)
			return
		}
		s.handleSocket(w, r, match[1])
		return
	}

	// Stable, shareable URL: redirect to whichever version is current, on the
	// origin that serves documents.
	if match := reRawCurrent.FindStringSubmatch(path); match != nil {
		entry, ok := s.store.get(match[1])
		if !ok {
			http.Error(w, "not found", http.StatusNotFound)
			return
		}
		http.Redirect(w, r,
			fmt.Sprintf("%s/raw/%s/%s.html", docsOrigin(r), entry.Slug, entry.SHA),
			http.StatusFound)
		return
	}

	// --- api -----------------------------------------------------------------
	if path == "/api/documents" && r.Method == http.MethodPost {
		s.handleUpload(w, r)
		return
	}

	// Listing is the one thing a link-holder must not be able to do: knowing
	// one document must not reveal the others, so it takes a publisher.
	if path == "/api/list" && (r.Method == http.MethodPost || r.Method == http.MethodGet) {
		if crossSiteRefused(r) {
			writeJSON(w, http.StatusForbidden, crossSiteRefusal())
			return
		}
		who, ok := s.publisher(w, r)
		if !ok {
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{
			"documents": s.visible(s.store.list(), who),
		})
		return
	}

	if match := reDeletePath.FindStringSubmatch(path); match != nil && r.Method == http.MethodPost {
		s.handleDelete(w, r, match[1])
		return
	}

	if match := reDocumentPath.FindStringSubmatch(path); match != nil && r.Method == http.MethodGet {
		entry, ok := s.store.get(match[1])
		if !ok {
			writeJSON(w, http.StatusNotFound, map[string]any{"error": "not found"})
			return
		}
		id := s.whoami(r)
		total, open := s.rooms.get(match[1]).counts()
		writeJSON(w, http.StatusOK, map[string]any{
			"slug": entry.Slug, "title": entry.Title, "sha": entry.SHA,
			"created_at": entry.CreatedAt, "updated_at": entry.UpdatedAt,
			"comment_count": total, "open_count": open,
			// Where the reader should frame this document from, and the only
			// origin it will accept messages from.
			"docs_origin": docsOrigin(r),
			// Whether this caller may delete anyone's comment here, per rule G.
			"can_moderate": entry.ownedBy(s.owner(r, id), id.ID),
		})
		return
	}

	// REST fallbacks, used when the socket is unavailable.
	if match := reCommentsPath.FindStringSubmatch(path); match != nil {
		s.handleComments(w, r, match[1])
		return
	}

	// --- the shell -----------------------------------------------------------
	page := path
	if _, static := s.shell[page]; !static {
		switch {
		case reDocsPage.MatchString(path):
			page = "/reader.html"
		case path == "/":
			page = "/index.html"
		}
	}
	if asset, ok := s.shell[page]; ok {
		s.issueVisitor(w, r, asset)
		writeAsset(w, asset)
		return
	}
	http.Error(w, "not found", http.StatusNotFound)
}

func (s *server) handleSocket(w http.ResponseWriter, r *http.Request, slug string) {
	// Browsers always send Origin on a WebSocket handshake and cannot be made
	// to attach a custom header to one, so this is rule A's WebSocket variant:
	// Origin alone, checked only when present.
	if wsOriginRefused(r) {
		http.Error(w, "cross-site request refused", http.StatusForbidden)
		return
	}
	// A room belongs to a document. Without this, any invented slug would
	// conjure one, and since the rate limiter counts per room, a new slug per
	// comment would also mean no rate limit at all.
	entry, exists := s.store.get(slug)
	if !exists {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	current := s.rooms.get(slug)
	// Reading is always open; writing is checked per message, so a reader who
	// may not comment still sees the thread live.
	id := s.whoami(r)
	author := s.commentAuthor(r, id)
	isOwner := entry.ownedBy(s.owner(r, id), id.ID)

	socket, err := wsUpgrade(w, r)
	if err != nil {
		http.Error(w, "expected a websocket upgrade", http.StatusBadRequest)
		return
	}
	address := clientAddress(r)
	current.attach(socket, address)
	defer func() {
		current.detach(socket)
		socket.close(1000, "")
	}()

	hello, err := json.Marshal(map[string]any{"type": "hello", "comments": current.snapshotFor(author, isOwner)})
	if err != nil || socket.writeText(hello) != nil {
		return
	}

	for {
		raw, err := socket.readMessage()
		if err != nil {
			return
		}
		var incoming message
		if json.Unmarshal(raw, &incoming) != nil {
			continue
		}
		result, ok := s.applyFrom(current, incoming, address, id, author, isOwner)
		if !ok {
			payload, err := json.Marshal(result)
			if err != nil || socket.writeText(payload) != nil {
				return
			}
			continue
		}
		current.broadcast(result)
	}
}

func (s *server) handleComments(w http.ResponseWriter, r *http.Request, slug string) {
	if !reSlug.MatchString(slug) {
		writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad slug"})
		return
	}
	entry, exists := s.store.get(slug)
	if !exists {
		writeJSON(w, http.StatusNotFound, map[string]any{"error": "not found"})
		return
	}
	current := s.rooms.get(slug)
	id := s.whoami(r)
	author := s.commentAuthor(r, id)
	isOwner := entry.ownedBy(s.owner(r, id), id.ID)

	switch r.Method {
	case http.MethodGet:
		writeJSON(w, http.StatusOK, map[string]any{"comments": current.snapshotFor(author, isOwner)})
	case http.MethodPost:
		if crossSiteRefused(r) {
			writeJSON(w, http.StatusForbidden, crossSiteRefusal())
			return
		}
		var incoming message
		if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&incoming); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad request"})
			return
		}
		result, ok := s.applyFrom(current, incoming, clientAddress(r), id, author, isOwner)
		if ok {
			current.broadcast(result)
			writeJSON(w, http.StatusOK, result)
			return
		}
		writeJSON(w, http.StatusBadRequest, result)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

// upload is a parsed publish request: a title, an optional exact slug asking
// to replace that document, and the HTML to store, however the body arrived.
type upload struct {
	title string
	slug  string
	html  string
}

func (s *server) handleUpload(w http.ResponseWriter, r *http.Request) {
	if crossSiteRefused(r) {
		writeJSON(w, http.StatusForbidden, crossSiteRefusal())
		return
	}
	// Checked before the body is read, so an unauthorised upload costs nothing.
	who, ok := s.publisher(w, r)
	if !ok {
		return
	}
	parsed, ok := s.readUpload(w, r)
	if !ok {
		return
	}

	base := slugify(parsed.slug)
	if base == "" {
		base = slugify(parsed.title)
	}
	if base == "" {
		writeJSON(w, http.StatusBadRequest, map[string]any{"error": "could not derive a slug"})
		return
	}
	// An exact slug that already exists is a replacement of that document, and
	// keeps its URL and its comments. Anything else is a new document, and gets
	// a random suffix so the link cannot be guessed from the title.
	// Someone else's document is not yours to replace, and guessing its slug
	// should not even tell you it is there: a title that collides with another
	// publisher's document simply becomes a new document of your own.
	key := base
	if existing, exists := s.store.get(base); !exists || !existing.ownedBy(who.Key, who.ID) {
		key = base + "-" + randomSuffix()
	}

	sum := sha256.Sum256([]byte(parsed.html))
	digest := hex.EncodeToString(sum[:])
	entry, err := s.store.put(key, parsed.title, digest, parsed.html, who.Key, who.ID)
	if err != nil {
		var quota *quotaError
		if errors.As(err, &quota) {
			writeJSON(w, quota.status, map[string]any{"error": quota.message})
			return
		}
		writeJSON(w, http.StatusInternalServerError, map[string]any{"error": "could not store the document"})
		return
	}
	// Comments survive the replacement; they re-anchor in the reader.
	writeJSON(w, http.StatusCreated, map[string]any{
		"slug": entry.Slug, "title": entry.Title, "sha": entry.SHA,
		"created_at": entry.CreatedAt, "updated_at": entry.UpdatedAt,
		"url": "/docs/" + entry.Slug,
	})
}

// readUpload parses a publish request's body, in either format it may
// arrive as, and applies the checks common to both: a title and some HTML are
// present, and the HTML is not over the size ceiling. It answers the request
// itself and returns false on any problem, so handleUpload only has to decide
// where to store what comes back.
func (s *server) readUpload(w http.ResponseWriter, r *http.Request) (upload, bool) {
	var title, slug, html string

	if strings.Contains(r.Header.Get("content-type"), "multipart/form-data") {
		// ParseMultipartForm's argument caps memory, not the request: anything
		// larger spills to temporary files, so without this an oversized body
		// would be written to disk in full before the HTML limit below is even
		// consulted. The slack covers the part headers and the other fields.
		r.Body = http.MaxBytesReader(w, r.Body, int64(config.MaxHTML)+multipartSlack)
		if err := r.ParseMultipartForm(multipartMemory); err != nil {
			var tooLarge *http.MaxBytesError
			if errors.As(err, &tooLarge) {
				writeJSON(w, http.StatusRequestEntityTooLarge,
					map[string]any{"error": "that upload is too large"})
				return upload{}, false
			}
			writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad upload"})
			return upload{}, false
		}
		defer func() {
			if r.MultipartForm != nil {
				_ = r.MultipartForm.RemoveAll()
			}
		}()
		title, slug = r.FormValue("title"), r.FormValue("slug")
		if file, header, err := r.FormFile("file"); err == nil {
			defer file.Close()
			raw, err := io.ReadAll(io.LimitReader(file, int64(config.MaxHTML)+1))
			if err != nil {
				writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad upload"})
				return upload{}, false
			}
			html = string(raw)

			// Markdown dropped on the page is rendered here, so what gets
			// stored is HTML like everything else.
			if isMarkdown(header.Filename) {
				if strings.TrimSpace(title) == "" {
					title = titleFromMarkdown(html)
				}
				rendered, err := renderMarkdownDocument(html, strings.TrimSpace(title))
				if err != nil {
					writeJSON(w, http.StatusBadRequest,
						map[string]any{"error": "could not render that markdown"})
					return upload{}, false
				}
				html = rendered
			}
		}
	} else {
		// JSON escaping can inflate the document, so the body is allowed to be
		// larger than the document limit; the real check is on the decoded
		// html below. Refusing early keeps a huge body from being read at all,
		// and says why rather than failing to parse.
		ceiling := int64(config.MaxHTML)*2 + 1024
		if r.ContentLength > ceiling {
			writeJSON(w, http.StatusRequestEntityTooLarge, map[string]any{"error": "document too large"})
			return upload{}, false
		}
		var body struct {
			Title string `json:"title"`
			Slug  string `json:"slug"`
			HTML  string `json:"html"`
		}
		if err := json.NewDecoder(io.LimitReader(r.Body, ceiling)).Decode(&body); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad request"})
			return upload{}, false
		}
		title, slug, html = body.Title, body.Slug, body.HTML
	}

	title = strings.TrimSpace(title)
	if title == "" || strings.TrimSpace(html) == "" {
		writeJSON(w, http.StatusBadRequest, map[string]any{"error": "title and html are required"})
		return upload{}, false
	}
	// Stripped of control characters the same way every other free-text field
	// is, then capped: the index holding it is read on nearly every request,
	// on both backends, so an unbounded title is a way to sink the whole
	// deployment. Refused rather than truncated, and before anything is
	// written, so a caller sees the limit rather than a silently shortened
	// title.
	title = clean(title, utf8.RuneCountInString(title))
	if utf8.RuneCountInString(title) > config.MaxTitle {
		writeJSON(w, http.StatusBadRequest, map[string]any{"error": "title too long"})
		return upload{}, false
	}
	if len(html) > config.MaxHTML {
		writeJSON(w, http.StatusRequestEntityTooLarge, map[string]any{"error": "document too large"})
		return upload{}, false
	}

	return upload{title: title, slug: slug, html: html}, true
}

func (s *server) handleDelete(w http.ResponseWriter, r *http.Request, slug string) {
	if crossSiteRefused(r) {
		writeJSON(w, http.StatusForbidden, crossSiteRefusal())
		return
	}
	if !reSlug.MatchString(slug) {
		writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad slug"})
		return
	}
	who, allowed := s.publisher(w, r)
	if !allowed {
		return
	}
	entry, ok := s.store.get(slug)
	// Another publisher's document answers exactly as a missing one does, so a
	// guessed slug reveals nothing.
	if !ok || !entry.ownedBy(who.Key, who.ID) {
		writeJSON(w, http.StatusNotFound, map[string]any{"error": "not found"})
		return
	}
	removed, err := s.deleteDocument(slug)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError,
			map[string]any{"error": "could not remove the document"})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"deleted": slug, "title": entry.Title, "versions_removed": removed,
	})
}

func (s *server) serveDocument(w http.ResponseWriter, r *http.Request, slug, digest string) {
	if !reSlug.MatchString(slug) || !reSHA.MatchString(digest) {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	raw, err := s.store.read(slug, digest)
	if err != nil {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	// The document runs on its own origin, with nothing of the reader's to
	// reach for, so it may run its own scripts: charts, maps, whatever it
	// shipped with. What it may not do is escape the frame or be framed by
	// anyone but the reader.
	header := w.Header()
	header.Set("content-type", "text/html; charset=utf-8")
	header.Set("content-security-policy",
		"default-src 'self' data: blob: https:; "+
			"script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https:; "+
			"style-src 'self' 'unsafe-inline' data: https:; "+
			"frame-ancestors "+readerOrigin(r)+"; "+
			"form-action 'none'; base-uri 'none'")
	header.Set("x-content-type-options", "nosniff")
	privacyHeaders(header)
	// Content-addressed path, so the bytes behind a URL never change.
	header.Set("cache-control", "public, max-age=31536000, immutable")
	_, _ = w.Write(withAgent(raw, readerOrigin(r)))
}

// withAgent appends the in-frame half of the reader to a document. The stored
// bytes are never modified; the script is added on the way out, and told which
// origin to talk back to.
func withAgent(document []byte, reader string) []byte {
	tag := []byte(fmt.Sprintf(`<script src="/agent.js?reader=%s"></script>`, url.QueryEscape(reader)))

	// Before </body> if there is one, so the document has parsed by the time
	// the agent runs; appended otherwise.
	lower := bytes.ToLower(document)
	if at := bytes.LastIndex(lower, []byte("</body>")); at >= 0 {
		out := make([]byte, 0, len(document)+len(tag))
		out = append(out, document[:at]...)
		out = append(out, tag...)
		return append(out, document[at:]...)
	}
	return append(document, tag...)
}

// whoami identifies the caller: a browser by its session cookie, the CLI by
// the GitHub token it sends as a bearer. Neither is required; the zero
// identity simply means nobody is signed in. A bearer is verified against
// GitHub's check-token endpoint (cached by tokenCache), and is never trusted
// at all when this deployment has no OAuth app configured to verify it
// against.
func (s *server) whoami(r *http.Request) identity {
	if header := r.Header.Get("Authorization"); strings.HasPrefix(header, "Bearer ") {
		if !s.app.configured() {
			return identity{}
		}
		return s.tokens.verify(s.app.checkToken, strings.TrimPrefix(header, "Bearer "))
	}
	if cookie, err := r.Cookie(cookieName(r, sessionCookie)); err == nil {
		return readSession(s.key, cookie.Value)
	}
	return identity{}
}

// caller is what an authorized write is attributed to: Key is the owner key
// a document's Publisher field is compared against (a lowercased GitHub
// login, a visitor: key, or "" for neither), and ID is the GitHub numeric
// account id when the caller is signed in, which entry.ownedBy prefers when
// an entry carries one.
type caller struct {
	Key string
	ID  string
}

// publisher answers the request itself when the caller may not publish, and
// otherwise returns the key and id that own whatever that caller uploads:
// their GitHub account, or their browser where publishing needs no account
// at all.
func (s *server) publisher(w http.ResponseWriter, r *http.Request) (caller, bool) {
	id := s.whoami(r)
	if s.publishers.allows(id.Login) {
		return caller{Key: s.owner(r, id), ID: id.ID}, true
	}
	if id.Login == "" {
		writeJSON(w, http.StatusUnauthorized, map[string]any{
			"error": "sign in with GitHub to publish",
		})
		return caller{}, false
	}
	writeJSON(w, http.StatusForbidden, map[string]any{
		"error": fmt.Sprintf("@%s may not publish here; this deployment allows %s",
			id.Login, s.publishers.describe()),
	})
	return caller{}, false
}

// owner is the key a caller's uploads belong to. A signed-in caller is their
// GitHub login. Where publishing needs no account there is still someone on
// the other end, so an anonymous caller is named by the visitor cookie the
// shell handed their browser: not an identity, but enough that one visitor's
// uploads are not another's to list, replace or delete.
//
// A caller with neither -- the CLI publishing to a deployment open to
// everyone -- owns nothing, and their uploads stay shared.
func (s *server) owner(r *http.Request, id identity) string {
	if id.Login != "" {
		return strings.ToLower(id.Login)
	}
	if cookie, err := r.Cookie(cookieName(r, visitorCookie)); err == nil {
		if token := readVisitor(s.key, cookie.Value); token != "" {
			return visitorPrefix + token
		}
	}
	return ""
}

// commentAuthor is the key a comment or reply is attributed to (see
// comment.Author): the signed-in account, or a digest of the visitor cookie
// so that key can decide who may delete a comment without turning the raw
// cookie -- which also names the caller's uploads -- into something a
// comment payload carries around.
func (s *server) commentAuthor(r *http.Request, id identity) string {
	if id.Login != "" {
		return "github:" + strings.ToLower(id.Login)
	}
	if cookie, err := r.Cookie(cookieName(r, visitorCookie)); err == nil {
		if token := readVisitor(s.key, cookie.Value); token != "" {
			sum := sha256.Sum256([]byte(token))
			return "visitor:" + hex.EncodeToString(sum[:])
		}
	}
	return ""
}

// visible narrows a listing to what one caller should see: the reserved
// examples, the documents that predate ownership, and their own uploads.
func (s *server) visible(entries []indexEntry, who caller) []indexEntry {
	mine := make([]indexEntry, 0, len(entries))
	for _, entry := range entries {
		if entry.Example || entry.ownedBy(who.Key, who.ID) {
			mine = append(mine, entry)
		}
	}
	return mine
}

// handleAuth serves the sign-in routes: the redirect to GitHub, the callback
// it returns to, signing out, and the two endpoints the page and the CLI ask
// about the current state.
func (s *server) handleAuth(w http.ResponseWriter, r *http.Request) bool {
	switch r.URL.Path {
	case "/auth/login":
		// Nothing to sign in to: this deployment is open to everyone and was
		// started without a GitHub OAuth app.
		if !s.app.configured() {
			http.Error(w, "this deployment has no sign-in: everyone may read, comment and publish",
				http.StatusNotFound)
			return true
		}
		state := randomToken()
		http.SetCookie(w, &http.Cookie{
			Name: cookieName(r, stateCookie),
			// The next URL is arbitrary caller-supplied text, so it is
			// URL-encoded before it rides beside the state token in one
			// cookie value; a next containing "|" or "&" could otherwise be
			// misread as part of the cookie's own structure.
			Value: state + "|" + url.QueryEscape(r.URL.Query().Get("next")),
			Path:  "/", MaxAge: 600, HttpOnly: true, SameSite: http.SameSiteLaxMode,
			Secure: requestScheme(r) == "https",
		})
		http.Redirect(w, r, s.app.authorizeURL(s.callbackURL(r), state), http.StatusFound)
		return true

	case "/auth/callback":
		cookie, err := r.Cookie(cookieName(r, stateCookie))
		if err != nil {
			http.Error(w, "sign-in expired; try again", http.StatusBadRequest)
			return true
		}
		state, encodedNext, _ := strings.Cut(cookie.Value, "|")
		// The state ties this callback to the redirect that started it, so a
		// link someone else crafted cannot sign you in as them.
		if state == "" || r.URL.Query().Get("state") != state {
			http.Error(w, "sign-in state did not match; try again", http.StatusBadRequest)
			return true
		}
		next, err := url.QueryUnescape(encodedNext)
		if err != nil {
			next = ""
		}
		token, err := s.app.exchange(r.URL.Query().Get("code"), s.callbackURL(r))
		if err != nil {
			http.Error(w, "github refused the sign-in: "+err.Error(), http.StatusBadRequest)
			return true
		}
		who, err := loginFor(token)
		if err != nil {
			http.Error(w, "github would not say who you are", http.StatusBadGateway)
			return true
		}
		http.SetCookie(w, &http.Cookie{
			Name: cookieName(r, sessionCookie), Value: signSession(s.key, who, time.Now().Add(sessionMaxAge)),
			Path: "/", MaxAge: int(sessionMaxAge.Seconds()), HttpOnly: true,
			// Behind a proxy that terminates TLS this connection is plain
			// HTTP, so r.TLS alone would ship the session cookie without
			// Secure on a deployment that is in fact HTTPS end to end.
			SameSite: http.SameSiteLaxMode, Secure: requestScheme(r) == "https",
		})
		http.SetCookie(w, &http.Cookie{Name: cookieName(r, stateCookie), Value: "", Path: "/", MaxAge: -1})
		next = localPath(next)
		http.Redirect(w, r, next, http.StatusFound)
		return true

	case "/auth/logout":
		// A GET here would be a plain link or a browser prefetch either could
		// trigger from a hostile page, and cookies alone do not stop that on a
		// same-site document host; POST plus rule A's checks below do.
		if r.Method != http.MethodPost {
			w.Header().Set("allow", "POST")
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return true
		}
		if crossSiteRefused(r) {
			writeJSON(w, http.StatusForbidden, crossSiteRefusal())
			return true
		}
		http.SetCookie(w, &http.Cookie{
			Name: cookieName(r, sessionCookie), Value: "", Path: "/", MaxAge: -1,
			HttpOnly: true, SameSite: http.SameSiteLaxMode, Secure: requestScheme(r) == "https",
		})
		writeJSON(w, http.StatusOK, map[string]any{"logged_out": true})
		return true

	case "/api/me":
		id := s.whoami(r)
		writeJSON(w, http.StatusOK, map[string]any{
			"login":               id.Login,
			"can_publish":         s.publishers.allows(id.Login),
			"can_comment":         s.commenters.allows(id.Login),
			"comments_need_login": !s.commenters.Public,
			// A wholly public deployment has no OAuth app, so there is
			// nothing to sign in to and the page hides the button.
			"can_sign_in": s.app.configured(),
			"publishers":  s.publishers.describe(),
			"commenters":  s.commenters.describe(),
		})
		return true

	// The client id is public by design; the CLI asks for it so `login` needs
	// no configuration of its own.
	case "/api/auth/config":
		writeJSON(w, http.StatusOK, map[string]any{"client_id": s.app.ClientID})
		return true
	}
	return false
}

func (s *server) callbackURL(r *http.Request) string {
	return requestScheme(r) + "://" + r.Host + "/auth/callback"
}

// localPath is where a sign-in may return to: somewhere on this site, and
// nowhere else. A value like "//elsewhere.example" starts with a slash but is
// read by browsers as an absolute URL, which would make the callback an open
// redirect, so the path is parsed and required to carry no scheme or host.
func localPath(next string) string {
	if next == "" || !strings.HasPrefix(next, "/") || strings.HasPrefix(next, "//") {
		return "/"
	}
	parsed, err := url.Parse(next)
	if err != nil || parsed.Scheme != "" || parsed.Host != "" {
		return "/"
	}
	target := parsed.RequestURI()
	if parsed.Fragment != "" {
		target += "#" + parsed.EscapedFragment()
	}
	return target
}

func firstOf(values ...string) string {
	for _, value := range values {
		if unquote(value) != "" {
			return unquote(value)
		}
	}
	return ""
}

// unquote drops surrounding quotes. A .env read by make keeps them, unlike a
// shell, and a client id wearing quotation marks is one GitHub has never heard
// of. Every other .env convention allows them, so accept them here.
func unquote(value string) string {
	trimmed := strings.TrimSpace(value)
	if len(trimmed) >= 2 {
		first, last := trimmed[0], trimmed[len(trimmed)-1]
		if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
			return strings.TrimSpace(trimmed[1 : len(trimmed)-1])
		}
	}
	return trimmed
}

func writeJSON(w http.ResponseWriter, status int, payload any) {
	w.Header().Set("content-type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(payload)
}

// clientAddress is what the rate limiter counts against. Behind a reverse
// proxy the peer is the proxy, so the first X-Forwarded-For entry is the
// client -- but only a peer that could be that proxy is believed. A header
// from a direct client is its own invention, and honouring it would let one
// address claim a fresh identity for every comment and never be limited.
func clientAddress(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		host = r.RemoteAddr
	}
	if forwarded := r.Header.Get("X-Forwarded-For"); forwarded != "" && localPeer(host) {
		if first := strings.TrimSpace(strings.Split(forwarded, ",")[0]); first != "" {
			return first
		}
	}
	return host
}

// localPeer is true for the addresses a reverse proxy in front of this process
// connects from: the loopback interface, or a private network alongside it.
func localPeer(host string) bool {
	address, err := netip.ParseAddr(host)
	if err != nil {
		return false
	}
	address = address.Unmap()
	return address.IsLoopback() || address.IsPrivate() || address.IsLinkLocalUnicast()
}

// applyFrom enforces the comment policy, then hands the message to the room.
// When commenting needs a GitHub account, the name on the comment is the
// verified login rather than whatever the client typed.
func (s *server) applyFrom(current *room, incoming message, address string, id identity, author string, isOwner bool) (map[string]any, bool) {
	if !s.commenters.allows(id.Login) {
		reason := "sign in with GitHub to comment"
		if id.Login != "" {
			reason = fmt.Sprintf("@%s may not comment here; this deployment allows %s",
				id.Login, s.commenters.describe())
		}
		return map[string]any{
			"type": "error", "message": reason, "temp_id": incoming.TempID,
		}, false
	}
	// A signed-in commenter is named by their account, whether or not signing
	// in was required. Only anonymous readers type a name.
	if id.Login != "" {
		incoming.Creator = id.Login
	}
	return current.apply(incoming, address, author, isOwner)
}

// visitorPrefix keeps a browser's key from ever colliding with a GitHub
// login, which cannot contain a colon.
const visitorPrefix = "visitor:"

// issueVisitor names a browser the first time it is served a page, so an
// upload it makes without signing in belongs to it and to nobody else. Only
// pages carry it: an image or a font is not where a session starts.
func (s *server) issueVisitor(w http.ResponseWriter, r *http.Request, asset shellFile) {
	if !strings.HasPrefix(asset.Type, "text/html") {
		return
	}
	// An unsigned cookie -- from before this server signed them, or forged --
	// verifies as absent, so it is simply replaced with a signed one.
	if cookie, err := r.Cookie(cookieName(r, visitorCookie)); err == nil && readVisitor(s.key, cookie.Value) != "" {
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name: cookieName(r, visitorCookie), Value: signVisitor(s.key, randomToken()), Path: "/",
		MaxAge: int((365 * 24 * time.Hour).Seconds()), HttpOnly: true,
		// r.TLS alone misses a deployment behind a proxy that terminates TLS
		// itself, the same reason the session cookie above uses requestScheme
		// rather than r.TLS.
		SameSite: http.SameSiteLaxMode, Secure: requestScheme(r) == "https",
	})
}

// writeAsset serves one shell file: decoded if it is a binary carried as
// base64, and cached for a year if its bytes never change.
func writeAsset(w http.ResponseWriter, asset shellFile) {
	body := []byte(asset.Body)
	if asset.Base64 {
		decoded, err := base64.StdEncoding.DecodeString(asset.Body)
		if err != nil {
			http.Error(w, "bad asset", http.StatusInternalServerError)
			return
		}
		body = decoded
	}
	w.Header().Set("content-type", asset.Type)
	// Every shell page -- index, reader, documentation -- is a place a hostile
	// site could otherwise iframe to phish against, since the reader carries a
	// session cookie. A document keeps its own CSP, set where it is served,
	// which already names the one origin allowed to frame it.
	if strings.HasPrefix(asset.Type, "text/html") {
		w.Header().Set("content-security-policy", "frame-ancestors 'none'")
	}
	privacyHeaders(w.Header())
	switch {
	// This response carries a freshly minted visitor cookie, and a shared
	// cache handing that same identity to the next browser would defeat the
	// point of having one.
	case w.Header().Get("set-cookie") != "":
		w.Header().Set("cache-control", "private, no-store")
	case asset.Immutable:
		w.Header().Set("cache-control", "public, max-age=31536000, immutable")
	default:
		w.Header().Set("cache-control", "public, max-age=300")
	}
	_, _ = w.Write(body)
}

// privacyHeaders keep an unlisted link unlisted. The slug is the only thing
// standing between a document and the public, and a URL is easy to spill:
// a link in the document sends it to whatever site the reader clicks through
// to, and a crawler that finds it once has it for good.
func privacyHeaders(header http.Header) {
	header.Set("referrer-policy", "no-referrer")
	header.Set("x-robots-tag", "noindex, nofollow, noarchive")
}
