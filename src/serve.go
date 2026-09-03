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
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"
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

func (s *server) deleteDocument(slug string) int {
	s.rooms.purge(slug)
	return s.store.remove(slug)
}

func (s *server) deleteExpired(now time.Time, retention time.Duration, from string) int {
	removed := 0
	cutoff := now.Add(-retention)
	for _, entry := range s.store.list() {
		stamp, err := entry.expiryTime(from)
		if err == nil && !stamp.After(cutoff) {
			s.deleteDocument(entry.Slug)
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
		login, ok := s.publisher(w, r)
		if !ok {
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{
			"documents": s.visible(s.store.list(), login),
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
		total, open := s.rooms.get(match[1]).counts()
		writeJSON(w, http.StatusOK, map[string]any{
			"slug": entry.Slug, "title": entry.Title, "sha": entry.SHA,
			"created_at": entry.CreatedAt, "updated_at": entry.UpdatedAt,
			"comment_count": total, "open_count": open,
			// Where the reader should frame this document from, and the only
			// origin it will accept messages from.
			"docs_origin": docsOrigin(r),
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
	current := s.rooms.get(slug)
	// Reading is always open; writing is checked per message, so a reader who
	// may not comment still sees the thread live.
	identity := s.whoami(r)

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

	hello, err := json.Marshal(map[string]any{"type": "hello", "comments": current.snapshot()})
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
		result, ok := s.applyFrom(current, incoming, address, identity)
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
	current := s.rooms.get(slug)

	switch r.Method {
	case http.MethodGet:
		writeJSON(w, http.StatusOK, map[string]any{"comments": current.snapshot()})
	case http.MethodPost:
		var incoming message
		if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&incoming); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad request"})
			return
		}
		result, ok := s.applyFrom(current, incoming, clientAddress(r), s.whoami(r))
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

func (s *server) handleUpload(w http.ResponseWriter, r *http.Request) {
	// Checked before the body is read, so an unauthorised upload costs nothing.
	login, ok := s.publisher(w, r)
	if !ok {
		return
	}
	var title, slug, html string

	if strings.Contains(r.Header.Get("content-type"), "multipart/form-data") {
		if err := r.ParseMultipartForm(32 << 20); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad upload"})
			return
		}
		title, slug = r.FormValue("title"), r.FormValue("slug")
		if file, header, err := r.FormFile("file"); err == nil {
			defer file.Close()
			raw, err := io.ReadAll(io.LimitReader(file, int64(config.MaxHTML)+1))
			if err != nil {
				writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad upload"})
				return
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
					return
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
			return
		}
		var body struct {
			Title string `json:"title"`
			Slug  string `json:"slug"`
			HTML  string `json:"html"`
		}
		if err := json.NewDecoder(io.LimitReader(r.Body, ceiling)).Decode(&body); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad request"})
			return
		}
		title, slug, html = body.Title, body.Slug, body.HTML
	}

	title = strings.TrimSpace(title)
	if title == "" || strings.TrimSpace(html) == "" {
		writeJSON(w, http.StatusBadRequest, map[string]any{"error": "title and html are required"})
		return
	}
	if len(html) > config.MaxHTML {
		writeJSON(w, http.StatusRequestEntityTooLarge, map[string]any{"error": "document too large"})
		return
	}

	base := slugify(slug)
	if base == "" {
		base = slugify(title)
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
	if existing, exists := s.store.get(base); !exists || !existing.ownedBy(login) {
		key = base + "-" + randomSuffix()
	}

	sum := sha256.Sum256([]byte(html))
	digest := hex.EncodeToString(sum[:])
	entry, err := s.store.put(key, title, digest, html, login)
	if err != nil {
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

func (s *server) handleDelete(w http.ResponseWriter, r *http.Request, slug string) {
	if !reSlug.MatchString(slug) {
		writeJSON(w, http.StatusBadRequest, map[string]any{"error": "bad slug"})
		return
	}
	login, allowed := s.publisher(w, r)
	if !allowed {
		return
	}
	entry, ok := s.store.get(slug)
	// Another publisher's document answers exactly as a missing one does, so a
	// guessed slug reveals nothing.
	if !ok || !entry.ownedBy(login) {
		writeJSON(w, http.StatusNotFound, map[string]any{"error": "not found"})
		return
	}
	removed := s.deleteDocument(slug)
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
// the GitHub token it sends as a bearer. Neither is required; an empty login
// simply means nobody is signed in.
func (s *server) whoami(r *http.Request) string {
	if header := r.Header.Get("Authorization"); strings.HasPrefix(header, "Bearer ") {
		return s.tokens.login(strings.TrimPrefix(header, "Bearer "))
	}
	if cookie, err := r.Cookie(sessionCookie); err == nil {
		return readSession(s.key, cookie.Value)
	}
	return ""
}

// publisher answers the request itself when the caller may not publish, and
// otherwise returns the key that owns whatever that caller uploads: their
// GitHub login, or their browser where publishing needs no account at all.
func (s *server) publisher(w http.ResponseWriter, r *http.Request) (string, bool) {
	login := s.whoami(r)
	if s.publishers.allows(login) {
		return s.owner(r, login), true
	}
	if login == "" {
		writeJSON(w, http.StatusUnauthorized, map[string]any{
			"error": "sign in with GitHub to publish",
		})
		return "", false
	}
	writeJSON(w, http.StatusForbidden, map[string]any{
		"error": fmt.Sprintf("@%s may not publish here; this deployment allows %s",
			login, s.publishers.describe()),
	})
	return "", false
}

// owner is the key a caller's uploads belong to. A signed-in caller is their
// GitHub login. Where publishing needs no account there is still someone on
// the other end, so an anonymous caller is named by the visitor cookie the
// shell handed their browser: not an identity, but enough that one visitor's
// uploads are not another's to list, replace or delete.
//
// A caller with neither -- the CLI publishing to a deployment open to
// everyone -- owns nothing, and their uploads stay shared.
func (s *server) owner(r *http.Request, login string) string {
	if login != "" {
		return strings.ToLower(login)
	}
	if cookie, err := r.Cookie(visitorCookie); err == nil && cookie.Value != "" {
		return visitorPrefix + cookie.Value
	}
	return ""
}

// visible narrows a listing to what one owner should see: the reserved
// examples, the documents that predate ownership, and their own uploads.
func (s *server) visible(entries []indexEntry, owner string) []indexEntry {
	mine := make([]indexEntry, 0, len(entries))
	for _, entry := range entries {
		if entry.Example || entry.ownedBy(owner) {
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
			Name: stateCookie, Value: state + "|" + r.URL.Query().Get("next"),
			Path: "/", MaxAge: 600, HttpOnly: true, SameSite: http.SameSiteLaxMode,
		})
		http.Redirect(w, r, s.app.authorizeURL(s.callbackURL(r), state), http.StatusFound)
		return true

	case "/auth/callback":
		cookie, err := r.Cookie(stateCookie)
		if err != nil {
			http.Error(w, "sign-in expired; try again", http.StatusBadRequest)
			return true
		}
		state, next, _ := strings.Cut(cookie.Value, "|")
		// The state ties this callback to the redirect that started it, so a
		// link someone else crafted cannot sign you in as them.
		if state == "" || r.URL.Query().Get("state") != state {
			http.Error(w, "sign-in state did not match; try again", http.StatusBadRequest)
			return true
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
			Name: sessionCookie, Value: signSession(s.key, who, time.Now().Add(sessionMaxAge)),
			Path: "/", MaxAge: int(sessionMaxAge.Seconds()), HttpOnly: true,
			SameSite: http.SameSiteLaxMode, Secure: r.TLS != nil,
		})
		http.SetCookie(w, &http.Cookie{Name: stateCookie, Value: "", Path: "/", MaxAge: -1})
		if next == "" || !strings.HasPrefix(next, "/") {
			next = "/"
		}
		http.Redirect(w, r, next, http.StatusFound)
		return true

	case "/auth/logout":
		http.SetCookie(w, &http.Cookie{Name: sessionCookie, Value: "", Path: "/", MaxAge: -1})
		http.Redirect(w, r, "/", http.StatusFound)
		return true

	case "/api/me":
		login := s.whoami(r)
		writeJSON(w, http.StatusOK, map[string]any{
			"login":               login,
			"can_publish":         s.publishers.allows(login),
			"can_comment":         s.commenters.allows(login),
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
	scheme := "http"
	if r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https" {
		scheme = "https"
	}
	return scheme + "://" + r.Host + "/auth/callback"
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
// proxy the peer is the proxy, so trust the first X-Forwarded-For entry.
func clientAddress(r *http.Request) string {
	if forwarded := r.Header.Get("X-Forwarded-For"); forwarded != "" {
		return strings.TrimSpace(strings.Split(forwarded, ",")[0])
	}
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

// applyFrom enforces the comment policy, then hands the message to the room.
// When commenting needs a GitHub account, the name on the comment is the
// verified login rather than whatever the client typed.
func (s *server) applyFrom(current *room, incoming message, address, identity string) (map[string]any, bool) {
	if !s.commenters.allows(identity) {
		reason := "sign in with GitHub to comment"
		if identity != "" {
			reason = fmt.Sprintf("@%s may not comment here; this deployment allows %s",
				identity, s.commenters.describe())
		}
		return map[string]any{
			"type": "error", "message": reason, "temp_id": incoming.TempID,
		}, false
	}
	// A signed-in commenter is named by their account, whether or not signing
	// in was required. Only anonymous readers type a name.
	if identity != "" {
		incoming.Creator = identity
	}
	return current.apply(incoming, address)
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
	if cookie, err := r.Cookie(visitorCookie); err == nil && cookie.Value != "" {
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name: visitorCookie, Value: randomToken(), Path: "/",
		MaxAge: int((365 * 24 * time.Hour).Seconds()), HttpOnly: true,
		SameSite: http.SameSiteLaxMode, Secure: r.TLS != nil,
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
