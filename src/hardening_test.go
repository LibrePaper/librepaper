package main

import (
	"bufio"
	"bytes"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// A room belongs to a document, so an invented slug gets nothing: no room, no
// comments file, and no rate-limit counter of its own to reset.
func TestCommentsOnAnUnknownDocumentAreRefused(t *testing.T) {
	server, instance := newTestServer(t)

	status, _ := postAs(t, "", server.URL, "/api/documents/no-such-doc/comments", message{
		Type: "comment", Body: "hello", Exact: "anything",
	})
	if status != http.StatusNotFound {
		t.Fatalf("commenting on a missing document: got %d, want 404", status)
	}

	response, err := http.Get(server.URL + "/api/documents/no-such-doc/comments")
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusNotFound {
		t.Fatalf("reading a missing document's comments: got %d, want 404", response.StatusCode)
	}

	if _, err := os.Stat(filepath.Join(instance.rooms.dir, "no-such-doc.json")); !os.IsNotExist(err) {
		t.Fatal("a refused comment left a room file behind")
	}
}

// A next= that starts with two slashes is an absolute URL to a browser, and
// following it after sign-in would walk the user off the site.
func TestLocalPathRefusesToLeaveTheSite(t *testing.T) {
	cases := map[string]string{
		"/docs/example":            "/docs/example",
		"/docs/x?a=1#top":          "/docs/x?a=1#top",
		"":                         "/",
		"//attacker.example":       "/",
		"///attacker.example":      "/",
		"https://attacker.example": "/",
		"docs/example":             "/",
	}
	for next, want := range cases {
		if got := localPath(next); got != want {
			t.Errorf("localPath(%q) = %q, want %q", next, got, want)
		}
	}
	if got := localPath(`/\attacker.example`); got == `/\attacker.example` {
		t.Error("localPath left a backslash that browsers read as a second slash")
	}
}

// The rate limiter counts against an address, so a header a direct client can
// write must not be able to supply it.
func TestForwardedForIsOnlyBelievedFromALocalPeer(t *testing.T) {
	request := httptest.NewRequest("POST", "/api/documents/x/comments", nil)
	request.RemoteAddr = "203.0.113.7:5000"
	request.Header.Set("X-Forwarded-For", "198.51.100.1")
	if got := clientAddress(request); got != "203.0.113.7" {
		t.Errorf("a remote peer's X-Forwarded-For was believed: got %q", got)
	}

	proxied := httptest.NewRequest("POST", "/api/documents/x/comments", nil)
	proxied.RemoteAddr = "127.0.0.1:5000"
	proxied.Header.Set("X-Forwarded-For", "198.51.100.1, 10.0.0.3")
	if got := clientAddress(proxied); got != "198.51.100.1" {
		t.Errorf("a local proxy's X-Forwarded-For was ignored: got %q", got)
	}
}

// An index that exists but cannot be parsed is not an empty store: starting
// empty would present every stored document as gone and let the next publish
// overwrite the real index.
func TestUnreadableIndexIsNotTreatedAsEmpty(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "index.json"), []byte("{not json"), 0o644); err != nil {
		t.Fatal(err)
	}
	// newStore exits the process on a corrupt index, so the check sits one
	// level down, where a test can see it without forking a child.
	if _, err := loadIndex(filepath.Join(dir, "index.json")); err == nil {
		t.Fatal("a corrupt index was accepted as an empty store")
	}
	if entries, err := loadIndex(filepath.Join(dir, "absent.json")); err != nil || len(entries) != 0 {
		t.Fatalf("a missing index should be an empty store: %v, %v", entries, err)
	}
}

// rawPost sends a POST with exactly the headers given, bypassing the
// X-Komodoc-Client header postAs always adds, so a test can construct
// exactly the cross-site shape rule A is meant to catch.
func rawPost(t *testing.T, base, path, cookie string, headers map[string]string, payload any) (int, map[string]any) {
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
	if cookie != "" {
		request.Header.Set("cookie", cookie)
	}
	for name, value := range headers {
		request.Header.Set(name, value)
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

// TestCrossSiteWritesAreRefused is rule A: a cookie-authenticated request to
// a state-changing route must be refused unless it looks same-origin. Two
// shapes below each fail one leg of that: a foreign Origin header, and the
// custom header a cross-origin browser request cannot attach without a
// preflight that is never granted.
func TestCrossSiteWritesAreRefused(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)

	shapes := map[string]map[string]string{
		"foreign origin":        {"Origin": "https://evil.example", "X-Komodoc-Client": "1"},
		"missing client header": {},
	}

	check := func(name, path string, cookie string, body any) {
		for shape, headers := range shapes {
			status, payload := rawPost(t, server.URL, path, cookie, headers, body)
			if status != http.StatusForbidden || payload["error"] != "cross-site request refused" {
				t.Errorf("%s (%s): got %d %v, want 403 cross-site request refused", name, shape, status, payload)
			}
		}
	}

	check("upload", "/api/documents", sessionAs(testPublisher),
		map[string]string{"title": "x", "html": "<p>x</p>"})
	check("delete", "/api/documents/"+slug+"/delete", sessionAs(testPublisher), nil)
	check("comments", "/api/documents/"+slug+"/comments", "",
		map[string]string{"type": "comment", "exact": "hello", "body": "hi"})
	check("list", "/api/list", sessionAs(testPublisher), map[string]any{})
	check("logout", "/auth/logout", sessionAs(testPublisher), nil)
}

// TestWebSocketForeignOriginIsRefused is rule A's WebSocket variant: a
// browser always sends Origin on the handshake and cannot be made to attach
// the custom header the other routes rely on, so Origin alone is checked.
func TestWebSocketForeignOriginIsRefused(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)

	address := strings.TrimPrefix(server.URL, "http://")
	conn, err := net.Dial("tcp", address)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(10 * time.Second))

	key := make([]byte, 16)
	_, _ = rand.Read(key)
	encoded := base64.StdEncoding.EncodeToString(key)
	request := fmt.Sprintf("GET /ws/%s HTTP/1.1\r\nHost: %s\r\nOrigin: https://evil.example\r\n"+
		"Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n",
		slug, address, encoded)
	if _, err := io.WriteString(conn, request); err != nil {
		t.Fatal(err)
	}

	response, err := http.ReadResponse(bufio.NewReader(conn), nil)
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != http.StatusForbidden {
		t.Fatalf("a websocket upgrade with a foreign Origin got %d, want 403", response.StatusCode)
	}
}

// TestOversizedTitleIsRefused is rule D: a title over max_title runes is
// refused before anything is written, rather than silently truncated.
func TestOversizedTitleIsRefused(t *testing.T) {
	server, _ := newTestServer(t)
	status, payload := post(t, server.URL, "/api/documents", map[string]string{
		"title": strings.Repeat("x", 300),
		"html":  "<p>hello</p>",
	})
	if status != http.StatusBadRequest || payload["error"] != "title too long" {
		t.Fatalf("a 300-rune title got %d %v, want 400 title too long", status, payload)
	}
}

// TestReplyLimitIsEnforced is rule D's other half: a comment's replies are
// capped so one thread cannot grow without bound.
func TestReplyLimitIsEnforced(t *testing.T) {
	original := config.RatePerHour
	config.RatePerHour = config.MaxReplies + 10
	t.Cleanup(func() { config.RatePerHour = original })

	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	path := "/api/documents/" + document["slug"].(string) + "/comments"

	status, payload := post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "body": "root",
	})
	if status != http.StatusOK {
		t.Fatalf("root comment got %d %v", status, payload)
	}
	commentID := payload["comment"].(map[string]any)["id"].(string)

	for i := 0; i < config.MaxReplies; i++ {
		status, payload := post(t, server.URL, path, map[string]any{
			"type": "reply", "comment_id": commentID, "body": fmt.Sprintf("reply %d", i),
		})
		if status != http.StatusOK {
			t.Fatalf("reply %d got %d %v, want 200", i, status, payload)
		}
	}

	status, payload = post(t, server.URL, path, map[string]any{
		"type": "reply", "comment_id": commentID, "body": "one too many",
	})
	if status != http.StatusBadRequest || payload["message"] != "this comment has reached its reply limit" {
		t.Fatalf("the %dth reply got %d %v, want the reply-limit refusal", config.MaxReplies+1, status, payload)
	}
}

// TestCommentDeleteAuthorization is rule H: a caller may delete their own
// comment and nothing else, except the document's owner, who may delete
// anything on it.
func TestCommentDeleteAuthorization(t *testing.T) {
	server, _ := newTestServer(t)
	// publishTestDocument publishes as testPublisher, via post(), so
	// testPublisher owns this document for the rest of the test.
	document := publishTestDocument(t, server.URL)
	path := "/api/documents/" + document["slug"].(string) + "/comments"

	status, payload := postAs(t, visitorAs("alpha"), server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "body": "alpha's comment", "creator": "Alpha",
	})
	if status != http.StatusOK {
		t.Fatalf("alpha's comment got %d %v", status, payload)
	}
	commentID := payload["comment"].(map[string]any)["id"].(string)

	// A stranger may not delete alpha's comment.
	status, payload = postAs(t, visitorAs("beta"), server.URL, path, map[string]any{
		"type": "delete", "comment_id": commentID,
	})
	if status != http.StatusBadRequest || payload["message"] != "you may only delete your own comments" {
		t.Fatalf("beta deleting alpha's comment got %d %v", status, payload)
	}

	// The document's owner may delete it anyway.
	status, payload = post(t, server.URL, path, map[string]any{
		"type": "delete", "comment_id": commentID,
	})
	if status != http.StatusOK {
		t.Fatalf("the document owner deleting alpha's comment got %d %v", status, payload)
	}

	// alpha may delete their own comment.
	status, payload = postAs(t, visitorAs("alpha"), server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "body": "alpha's second comment", "creator": "Alpha",
	})
	if status != http.StatusOK {
		t.Fatalf("alpha's second comment got %d %v", status, payload)
	}
	commentID = payload["comment"].(map[string]any)["id"].(string)
	status, payload = postAs(t, visitorAs("alpha"), server.URL, path, map[string]any{
		"type": "delete", "comment_id": commentID,
	})
	if status != http.StatusOK {
		t.Fatalf("alpha deleting their own comment got %d %v", status, payload)
	}
}

// TestResolveCountsAgainstTheRateLimit is rule H: resolving a comment now
// costs a rate-limit slot the same as posting one does.
func TestResolveCountsAgainstTheRateLimit(t *testing.T) {
	original := config.RatePerHour
	config.RatePerHour = 2
	t.Cleanup(func() { config.RatePerHour = original })

	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	path := "/api/documents/" + document["slug"].(string) + "/comments"

	// The comment itself is the first of the two slots this test allows.
	status, payload := post(t, server.URL, path, map[string]any{
		"type": "comment", "exact": "hello", "body": "root",
	})
	if status != http.StatusOK {
		t.Fatalf("comment got %d %v", status, payload)
	}
	commentID := payload["comment"].(map[string]any)["id"].(string)

	status, payload = post(t, server.URL, path, map[string]any{
		"type": "resolve", "comment_id": commentID, "resolved": true,
	})
	if status != http.StatusOK {
		t.Fatalf("the second of two allowed writes got %d %v, want 200", status, payload)
	}

	status, payload = post(t, server.URL, path, map[string]any{
		"type": "resolve", "comment_id": commentID, "resolved": false,
	})
	if status != http.StatusBadRequest || payload["message"] != "too many comments from this address; try later" {
		t.Fatalf("a resolve past the rate limit got %d %v, want the rate-limit refusal", status, payload)
	}
}

// TestRateKeyCollapsesIPv6ToASixtyFourPrefix is rule E: two addresses in the
// same /64 -- the block an ISP typically hands one customer -- share a rate
// limit key, while an IPv4 address is used whole.
func TestRateKeyCollapsesIPv6ToASixtyFourPrefix(t *testing.T) {
	cases := map[string]string{
		"2001:db8:1:1::1":      "2001:db8:1:1",
		"2001:db8:1:1:ffff::9": "2001:db8:1:1",
		"2001:db8:1:2::1":      "2001:db8:1:2",
		"203.0.113.7":          "203.0.113.7",
		"::1":                  "0:0:0:0",
		"not-an-address":       "not-an-address",
	}
	for address, want := range cases {
		if got := rateKey(address); got != want {
			t.Errorf("rateKey(%q) = %q, want %q", address, got, want)
		}
	}
}

// TestBearerTokenRejectedWhenAppUnconfigured is rule F: without an OAuth
// client id and secret to verify a bearer against, one is never trusted, no
// matter what it looks like.
func TestBearerTokenRejectedWhenAppUnconfigured(t *testing.T) {
	dir := t.TempDir()
	instance := &server{
		store:      newStore(dir),
		rooms:      newRoomSet(filepath.Join(dir, "comments")),
		shell:      loadShell(),
		app:        githubApp{},
		key:        testKey,
		tokens:     newTokenCache(),
		publishers: parsePolicy("any"),
		commenters: parsePolicy("anyone"),
	}
	srv := httptest.NewServer(instance)
	t.Cleanup(srv.Close)

	request, err := http.NewRequest("GET", srv.URL+"/api/me", nil)
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer whatever-looks-like-a-token")
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	var payload map[string]any
	_ = json.NewDecoder(response.Body).Decode(&payload)
	if payload["login"] != "" {
		t.Fatalf("a bearer token was trusted with no OAuth app configured: %v", payload)
	}
}

// TestHTTPSRequestsOnlyReadTheHostPrefixedSessionCookie is rule B: on a
// request that looks like HTTPS, a plain-named session cookie -- exactly what
// a same-site document could plant -- must never be fallen back to.
func TestHTTPSRequestsOnlyReadTheHostPrefixedSessionCookie(t *testing.T) {
	server, _ := newTestServer(t)

	get := func(cookie string) map[string]any {
		t.Helper()
		request, err := http.NewRequest("GET", server.URL+"/api/me", nil)
		if err != nil {
			t.Fatal(err)
		}
		request.Header.Set("X-Forwarded-Proto", "https")
		request.Header.Set("cookie", cookie)
		response, err := http.DefaultClient.Do(request)
		if err != nil {
			t.Fatal(err)
		}
		defer response.Body.Close()
		var payload map[string]any
		_ = json.NewDecoder(response.Body).Decode(&payload)
		return payload
	}

	if payload := get(sessionAs(testPublisher)); payload["login"] != "" {
		t.Fatalf("a plain-named session cookie was read on an HTTPS request: %v", payload)
	}
	if payload := get(hostCookiePrefix + sessionAs(testPublisher)); payload["login"] != testPublisher {
		t.Fatalf("the __Host- session cookie was not read on an HTTPS request: %v", payload)
	}
}

// TestLogoutIsPostOnly checks rule A's method half: a GET to /auth/logout --
// a plain link, or a browser's own prefetch, either of which a hostile page
// could trigger -- is refused outright, before the cross-site checks even
// run.
func TestLogoutIsPostOnly(t *testing.T) {
	server, _ := newTestServer(t)

	request, err := http.NewRequest("GET", server.URL+"/auth/logout", nil)
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("cookie", sessionAs(testPublisher))
	client := &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error {
		return http.ErrUseLastResponse
	}}
	response, err := client.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusMethodNotAllowed {
		t.Fatalf("GET /auth/logout returned %d, want 405", response.StatusCode)
	}

	status, payload := postAs(t, sessionAs(testPublisher), server.URL, "/auth/logout", nil)
	if status != http.StatusOK || payload["logged_out"] != true {
		t.Fatalf("POST /auth/logout returned %d %v, want 200 logged_out", status, payload)
	}
}

// TestCommentAuthorIsPersistedButNeverSentToClients checks the split the
// design settled on: comment.Author and reply.Author carry json:"-" so
// nothing that marshals a comment or reply for a client -- a broadcast, a
// snapshot -- can leak it by accident, while room.save/load still round-trip
// it through storedComment/storedReply, since a delete after a restart still
// has to know who wrote what.
func TestCommentAuthorIsPersistedButNeverSentToClients(t *testing.T) {
	dir := t.TempDir()
	rooms := newRoomSet(filepath.Join(dir, "comments"))
	current := rooms.get("doc-1")

	result, ok := current.apply(message{Type: "comment", Exact: "hello", Body: "hi"}, "", "github:vincent", false)
	if !ok {
		t.Fatalf("comment was refused: %v", result)
	}
	added, ok := result["comment"].(*comment)
	if !ok || added.Author != "github:vincent" {
		t.Fatalf("author was not set on the in-memory comment: %#v", result["comment"])
	}

	// A broadcast marshals the comment directly; it must carry no author.
	raw, err := json.Marshal(added)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(raw), "author") {
		t.Fatalf("the broadcast comment leaked its author: %s", raw)
	}

	// A fresh room, as a restart would see, still knows who wrote it.
	reloaded := newRoomSet(filepath.Join(dir, "comments")).get("doc-1")
	snapshot := reloaded.snapshot()
	if len(snapshot) != 1 || snapshot[0].Author != "github:vincent" {
		t.Fatalf("author did not survive a reload: %#v", snapshot)
	}

	// The per-caller view a client actually receives excludes it too, and
	// marks the comment deletable for the author who wrote it.
	view := reloaded.snapshotFor("github:vincent", false)
	encoded, err := json.Marshal(view)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(encoded), `"author"`) {
		t.Fatalf("snapshotFor leaked author: %s", encoded)
	}
	if len(view) != 1 || !view[0].Deletable {
		t.Fatalf("the comment's own author should find it deletable: %#v", view)
	}
	if strangers := reloaded.snapshotFor("github:someone-else", false); len(strangers) != 1 || strangers[0].Deletable {
		t.Fatalf("a stranger should not find it deletable: %#v", strangers)
	}
}
