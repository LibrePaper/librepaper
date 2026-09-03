package main

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
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
