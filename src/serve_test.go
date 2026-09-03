package main

import (
	"bufio"
	"bytes"
	"crypto/rand"
	"crypto/sha1"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The tests sign in as this GitHub login by forging a session cookie, which
// is what the real sign-in produces at the end of the OAuth dance.
const testPublisher = "vincent"

// A fixed signing key, so a test can mint the session cookie a real sign-in
// would have produced without going near GitHub.
var testKey = []byte("0123456789abcdef0123456789abcdef")

func newTestServer(t *testing.T) (*httptest.Server, *server) {
	t.Helper()
	dir := t.TempDir()
	instance := &server{
		store:      newStore(dir),
		rooms:      newRoomSet(filepath.Join(dir, "comments")),
		shell:      loadShell(),
		app:        githubApp{ClientID: "test-client", ClientSecret: "test-secret"},
		key:        testKey,
		tokens:     newTokenCache(),
		publishers: parsePolicy(testPublisher),
		commenters: parsePolicy("anyone"),
	}
	http := httptest.NewServer(instance)
	t.Cleanup(http.Close)
	return http, instance
}

// sessionAs is the cookie a browser carries after signing in as login. The
// login itself doubles as the fake GitHub numeric id, which is fine for a
// test: it only has to be stable and distinct per login, the way a real
// account's id is.
func sessionAs(login string) string {
	return sessionCookie + "=" + signSession(testKey, identity{Login: login, ID: login}, time.Now().Add(time.Hour))
}

// post sends a request signed in as the allowed publisher.
func post(t *testing.T, base, path string, payload any) (int, map[string]any) {
	t.Helper()
	return postAs(t, sessionAs(testPublisher), base, path, payload)
}

// postAs sends a request carrying whatever cookie is given, including none.
func postAs(t *testing.T, cookie, base, path string, payload any) (int, map[string]any) {
	t.Helper()
	body, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	request, err := http.NewRequest("POST", base+path, bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("content-type", "application/json")
	// A browser cannot attach a custom header to a cross-origin request
	// without a preflight that is never granted, so this is what marks a
	// same-origin request under rule A; every cookie-authenticated call in
	// these tests is one.
	request.Header.Set("X-Komodoc-Client", "1")
	if cookie != "" {
		request.Header.Set("cookie", cookie)
	}
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	var decoded map[string]any
	_ = json.NewDecoder(response.Body).Decode(&decoded)
	return response.StatusCode, decoded
}

func publishTestDocument(t *testing.T, base string) map[string]any {
	t.Helper()
	status, document := post(t, base, "/api/documents", map[string]string{
		"title": "My Paper",
		"html":  "<!doctype html><p>hello world</p>",
	})
	if status != 201 {
		t.Fatalf("upload returned %d: %v", status, document)
	}
	return document
}

func TestUploadNeedsASignedInPublisher(t *testing.T) {
	server, _ := newTestServer(t)

	status, payload := postAs(t, "", server.URL, "/api/documents",
		map[string]string{"title": "x", "html": "<p>x</p>"})
	if status != 401 || payload["error"] != "sign in with GitHub to publish" {
		t.Fatalf("anonymous upload got %d %v, want 401", status, payload)
	}

	// Signed in, but not one of the allowed logins.
	status, payload = postAs(t, sessionAs("stranger"), server.URL, "/api/documents",
		map[string]string{"title": "x", "html": "<p>x</p>"})
	if status != 403 || !strings.Contains(text(payload["error"]), "@stranger may not publish") {
		t.Fatalf("stranger's upload got %d %v, want 403", status, payload)
	}

	// A forged cookie is not a sign-in.
	status, _ = postAs(t, sessionCookie+"=nonsense.nonsense", server.URL, "/api/documents",
		map[string]string{"title": "x", "html": "<p>x</p>"})
	if status != 401 {
		t.Fatalf("forged cookie got %d, want 401", status)
	}
}

func TestPublishThenServeDocument(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)

	slug, _ := document["slug"].(string)
	if !strings.HasPrefix(slug, "my-paper-") || len(slug) != len("my-paper-")+config.SuffixLength {
		t.Fatalf("slug %q should be the title plus a random suffix", slug)
	}
	if document["url"] != "/docs/"+slug {
		t.Fatalf("url %v does not match slug %q", document["url"], slug)
	}

	sha, _ := document["sha"].(string)
	version := "/raw/" + slug + "/" + sha + ".html"

	// Asking the reader's own host for a document sends you to the other one.
	stop := &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error {
		return http.ErrUseLastResponse
	}}
	redirect, err := stop.Get(server.URL + version)
	if err != nil {
		t.Fatal(err)
	}
	redirect.Body.Close()
	if redirect.StatusCode != 302 || !strings.Contains(redirect.Header.Get("location"), "docs.") {
		t.Fatalf("a document on the reader host should redirect to the document origin, got %d %q",
			redirect.StatusCode, redirect.Header.Get("location"))
	}

	// On the document host it is served, with the agent added and a CSP that
	// lets it run its own scripts while pinning who may frame it.
	response := onDocsHost(t, server, version)
	defer response.Body.Close()
	body, _ := io.ReadAll(response.Body)
	if !strings.Contains(string(body), "hello world") {
		t.Fatalf("document body was %q", body)
	}
	if !strings.Contains(string(body), `<script src="/agent.js?reader=`) {
		t.Fatalf("the in-frame agent was not injected: %q", body)
	}
	csp := response.Header.Get("content-security-policy")
	if !strings.Contains(csp, "script-src 'self'") || !strings.Contains(csp, "frame-ancestors http://") {
		t.Fatalf("document CSP was %q", csp)
	}

	// The agent itself is served from the document origin, and nothing else is.
	if agent := onDocsHost(t, server, "/agent.js"); agent.StatusCode != 200 {
		t.Fatalf("/agent.js on the document host returned %d", agent.StatusCode)
	}
	for _, path := range []string{"/", "/api/me", "/docs/" + slug, "/komodoc.css"} {
		if leak := onDocsHost(t, server, path); leak.StatusCode != 404 {
			t.Fatalf("%s should not exist on the document origin, got %d", path, leak.StatusCode)
		}
	}

	// The stable URL redirects to the current version, on the document origin.
	redirect, err = stop.Get(server.URL + "/raw/" + slug)
	if err != nil {
		t.Fatal(err)
	}
	redirect.Body.Close()
	if redirect.StatusCode != 302 || !strings.HasSuffix(redirect.Header.Get("location"), version) {
		t.Fatalf("got %d %q", redirect.StatusCode, redirect.Header.Get("location"))
	}
}

// onDocsHost asks the same server as if it were the document hostname, which
// is how the split is exercised without any DNS.
func onDocsHost(t *testing.T, server *httptest.Server, path string) *http.Response {
	t.Helper()
	request, err := http.NewRequest("GET", server.URL+path, nil)
	if err != nil {
		t.Fatal(err)
	}
	request.Host = "docs." + request.Host
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	return response
}

func TestRepublishKeepsSlugAndComments(t *testing.T) {
	server, _ := newTestServer(t)
	first := publishTestDocument(t, server.URL)
	slug := first["slug"].(string)

	socket := dialWebSocket(t, server.URL, slug)
	defer socket.Close()
	readSocketMessage(t, socket) // hello
	writeSocketMessage(t, socket, map[string]string{
		"type": "comment", "exact": "hello", "body": "a note", "creator": "Reader",
	})
	readSocketMessage(t, socket)

	status, second := post(t, server.URL, "/api/documents", map[string]string{
		"title": "My Paper", "slug": slug,
		"html": "<!doctype html><p>hello revised world</p>",
	})
	if status != 201 {
		t.Fatalf("republish returned %d: %v", status, second)
	}
	if second["slug"] != slug {
		t.Fatalf("republish changed the slug to %v", second["slug"])
	}
	if second["sha"] == first["sha"] {
		t.Fatal("republish should store a new version")
	}
	if second["created_at"] != first["created_at"] {
		t.Fatal("republish should keep the original creation time")
	}

	response, err := http.Get(server.URL + "/api/documents/" + slug)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	var document map[string]any
	_ = json.NewDecoder(response.Body).Decode(&document)
	if document["comment_count"] != float64(1) {
		t.Fatalf("comment did not survive the republish: %v", document)
	}
}

func TestCommentsBroadcastToEveryReader(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)

	author := dialWebSocket(t, server.URL, slug)
	defer author.Close()
	watcher := dialWebSocket(t, server.URL, slug)
	defer watcher.Close()

	for _, socket := range []net.Conn{author, watcher} {
		hello := readSocketMessage(t, socket)
		if hello["type"] != "hello" {
			t.Fatalf("first frame was %v, want hello", hello["type"])
		}
	}

	writeSocketMessage(t, author, map[string]string{
		"type": "comment", "exact": "hello", "body": "a note", "creator": "Reader", "temp_id": "t1",
	})

	// The sender gets the echo too, which is how it reconciles the row it drew
	// optimistically.
	for _, socket := range []net.Conn{author, watcher} {
		event := readSocketMessage(t, socket)
		if event["type"] != "comment" {
			t.Fatalf("got %v, want a comment event", event)
		}
		comment := event["comment"].(map[string]any)
		if comment["body"] != "a note" || comment["creator"] != "Reader" || comment["exact"] != "hello" {
			t.Fatalf("comment came back as %v", comment)
		}
		if comment["resolved"] != false || comment["seq"] != float64(1) {
			t.Fatalf("comment came back as %v", comment)
		}
	}

	// Replies and resolves reach everyone the same way.
	identifier := ""
	{
		response, err := http.Get(server.URL + "/api/documents/" + slug + "/comments")
		if err != nil {
			t.Fatal(err)
		}
		defer response.Body.Close()
		var listing struct {
			Comments []comment `json:"comments"`
		}
		_ = json.NewDecoder(response.Body).Decode(&listing)
		if len(listing.Comments) != 1 {
			t.Fatalf("REST listing returned %d comments", len(listing.Comments))
		}
		identifier = listing.Comments[0].ID
	}

	writeSocketMessage(t, author, map[string]any{
		"type": "resolve", "comment_id": identifier, "resolved": true,
	})
	event := readSocketMessage(t, watcher)
	if event["type"] != "resolve" || event["resolved"] != true || event["resolved_at"] == nil {
		t.Fatalf("resolve came back as %v", event)
	}
}

func TestCommentValidation(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)

	socket := dialWebSocket(t, server.URL, slug)
	defer socket.Close()
	readSocketMessage(t, socket)

	cases := []struct {
		name    string
		message map[string]string
		want    string
	}{
		{"no body", map[string]string{"type": "comment", "exact": "hello"}, "comment body is required"},
		{"no anchor", map[string]string{"type": "comment", "body": "x"}, "select some text or part of a figure to comment on"},
		{"unknown type", map[string]string{"type": "wat", "body": "x"}, "unknown message type"},
		{"unknown comment", map[string]string{"type": "reply", "comment_id": "nope", "body": "x"}, "unknown comment"},
	}
	for _, testCase := range cases {
		writeSocketMessage(t, socket, testCase.message)
		event := readSocketMessage(t, socket)
		if event["type"] != "error" || event["message"] != testCase.want {
			t.Fatalf("%s: got %v, want error %q", testCase.name, event, testCase.want)
		}
	}
}

func TestListingAndDelete(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)

	// Knowing a document's link must not reveal the others.
	if status, payload := postAs(t, "", server.URL, "/api/list", map[string]any{}); status != 401 {
		t.Fatalf("anonymous listing returned %d %v, want 401", status, payload)
	}
	status, payload := post(t, server.URL, "/api/list", map[string]any{})
	if status != 200 || len(payload["documents"].([]any)) != 1 {
		t.Fatalf("listing returned %d %v", status, payload)
	}

	status, deleted := post(t, server.URL, "/api/documents/"+slug+"/delete", map[string]any{})
	if status != 200 || deleted["deleted"] != slug || deleted["versions_removed"] != float64(1) {
		t.Fatalf("delete returned %d %v", status, deleted)
	}
	response, err := http.Get(server.URL + "/api/documents/" + slug)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != 404 {
		t.Fatalf("document still present after delete (%d)", response.StatusCode)
	}
}

func TestShellRoutes(t *testing.T) {
	server, _ := newTestServer(t)
	for path, want := range map[string]string{
		"/":              "<!doctype html",
		"/docs/anything": "<!doctype html",
		"/anchor.js":     "export",
	} {
		response, err := http.Get(server.URL + path)
		if err != nil {
			t.Fatal(err)
		}
		body, _ := io.ReadAll(response.Body)
		response.Body.Close()
		if response.StatusCode != 200 || !strings.Contains(strings.ToLower(string(body)), want) {
			t.Fatalf("%s returned %d, body did not contain %q", path, response.StatusCode, want)
		}
	}
}

// --- a minimal websocket client, enough to drive the server ----------------

func dialWebSocket(t *testing.T, base, slug string) net.Conn {
	t.Helper()
	address := strings.TrimPrefix(base, "http://")
	conn, err := net.Dial("tcp", address)
	if err != nil {
		t.Fatal(err)
	}
	_ = conn.SetDeadline(time.Now().Add(10 * time.Second))

	key := make([]byte, 16)
	_, _ = rand.Read(key)
	encoded := base64.StdEncoding.EncodeToString(key)
	request := fmt.Sprintf("GET /ws/%s HTTP/1.1\r\nHost: %s\r\nUpgrade: websocket\r\n"+
		"Connection: Upgrade\r\nSec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n",
		slug, address, encoded)
	if _, err := io.WriteString(conn, request); err != nil {
		t.Fatal(err)
	}

	reader := bufio.NewReader(conn)
	response, err := http.ReadResponse(reader, nil)
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != 101 {
		t.Fatalf("handshake returned %d", response.StatusCode)
	}
	expected := sha1Base64(encoded + wsGUID)
	if response.Header.Get("Sec-WebSocket-Accept") != expected {
		t.Fatalf("bad Sec-WebSocket-Accept: %q", response.Header.Get("Sec-WebSocket-Accept"))
	}
	// The handshake may have buffered the first frame along with the response,
	// so the same reader has to be used for the frames that follow.
	readers[conn] = reader
	return conn
}

var readers = map[net.Conn]*bufio.Reader{}

func sha1Base64(input string) string {
	sum := sha1.Sum([]byte(input))
	return base64.StdEncoding.EncodeToString(sum[:])
}

func writeSocketMessage(t *testing.T, conn net.Conn, payload any) {
	t.Helper()
	body, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	frame := []byte{0x81}
	size := len(body)
	switch {
	case size < 126:
		frame = append(frame, byte(size)|0x80)
	default:
		frame = append(frame, 126|0x80, byte(size>>8), byte(size))
	}
	var mask [4]byte
	_, _ = rand.Read(mask[:])
	frame = append(frame, mask[:]...)
	for i, b := range body {
		frame = append(frame, b^mask[i%4])
	}
	if _, err := conn.Write(frame); err != nil {
		t.Fatal(err)
	}
}

func readSocketMessage(t *testing.T, conn net.Conn) map[string]any {
	t.Helper()
	reader := readers[conn]
	var header [2]byte
	if _, err := io.ReadFull(reader, header[:]); err != nil {
		t.Fatal(err)
	}
	length := uint64(header[1] & 0x7F)
	switch length {
	case 126:
		var extended [2]byte
		if _, err := io.ReadFull(reader, extended[:]); err != nil {
			t.Fatal(err)
		}
		length = uint64(binary.BigEndian.Uint16(extended[:]))
	case 127:
		var extended [8]byte
		if _, err := io.ReadFull(reader, extended[:]); err != nil {
			t.Fatal(err)
		}
		length = binary.BigEndian.Uint64(extended[:])
	}
	if header[1]&0x80 != 0 {
		t.Fatal("server frames must not be masked")
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(reader, payload); err != nil {
		t.Fatal(err)
	}
	var decoded map[string]any
	if err := json.Unmarshal(payload, &decoded); err != nil {
		t.Fatalf("frame was not JSON: %q", payload)
	}
	return decoded
}

func TestOversizedUploadIsRefusedBySize(t *testing.T) {
	server, _ := newTestServer(t)
	status, payload := post(t, server.URL, "/api/documents", map[string]string{

		"title": "Big",
		"html":  "<p>" + strings.Repeat("x", config.MaxHTML) + "</p>",
	})
	if status != 413 || payload["error"] != "document too large" {
		t.Fatalf("got %d %v, want 413 document too large", status, payload)
	}
}

func TestDocumentOriginIsIsolated(t *testing.T) {
	server, instance := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)

	// The reader is told where to frame documents from, and it is not itself.
	response, err := http.Get(server.URL + "/api/documents/" + slug)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	var payload map[string]any
	_ = json.NewDecoder(response.Body).Decode(&payload)
	origin := text(payload["docs_origin"])
	if !strings.HasPrefix(origin, "http://docs.") {
		t.Fatalf("docs_origin was %q", origin)
	}
	if strings.TrimPrefix(origin, "http://docs.") == "" || origin == server.URL {
		t.Fatalf("documents must not be framed from the reader's own origin: %q", origin)
	}

	// A session cookie counts for nothing on the document origin: it serves no
	// API at all, so there is nothing there to authorise.
	request, _ := http.NewRequest("POST", server.URL+"/api/documents", strings.NewReader("{}"))
	request.Host = "docs." + request.Host
	request.Header.Set("cookie", sessionAs(testPublisher))
	refused, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer refused.Body.Close()
	if refused.StatusCode != 404 {
		t.Fatalf("the document origin answered the upload API with %d", refused.StatusCode)
	}
	_ = instance
}
