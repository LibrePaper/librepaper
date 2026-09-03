package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

// Where publishing takes a GitHub account, a publisher sees the reserved
// examples and their own uploads, and nobody else's document is theirs to
// replace or delete.

// anyPublisherServer is the sandbox shape: any signed-in GitHub account may
// publish, so several publishers share one deployment.
func anyPublisherServer(t *testing.T) (*httptest.Server, *server) {
	t.Helper()
	http, instance := newTestServer(t)
	instance.publishers = parsePolicy("any")
	return http, instance
}

// publishAs uploads a document as one publisher, returning its slug. An
// explicit slug is what asks to replace an existing document.
func publishAs(t *testing.T, base, login, title string, slug ...string) string {
	t.Helper()
	body := map[string]string{"title": title, "html": "<p>" + title + "</p>"}
	if len(slug) > 0 {
		body["slug"] = slug[0]
	}
	status, document := postAs(t, sessionAs(login), base, "/api/documents", body)
	if status != 201 {
		t.Fatalf("upload as @%s returned %d: %v", login, status, document)
	}
	return text(document["slug"])
}

// visitorAs is the cookie the shell hands a browser that has not signed in:
// signed, as issueVisitor would mint it, so owner() accepts it.
func visitorAs(id string) string {
	return visitorCookie + "=" + signVisitor(testKey, id)
}

// publishWith uploads carrying whatever cookie is given, including none.
func publishWith(t *testing.T, base, cookie, title string, slug ...string) string {
	t.Helper()
	body := map[string]string{"title": title, "html": "<p>" + title + "</p>"}
	if len(slug) > 0 {
		body["slug"] = slug[0]
	}
	status, document := postAs(t, cookie, base, "/api/documents", body)
	if status != 201 {
		t.Fatalf("upload returned %d: %v", status, document)
	}
	return text(document["slug"])
}

// slugsVisibleTo is what /api/list shows one signed-in publisher.
func slugsVisibleTo(t *testing.T, base, login string) []string {
	t.Helper()
	return slugsVisibleWith(t, base, sessionAs(login))
}

// slugsVisibleWith is what /api/list shows a caller carrying one cookie.
func slugsVisibleWith(t *testing.T, base, cookie string) []string {
	t.Helper()
	status, payload := postAs(t, cookie, base, "/api/list", nil)
	if status != 200 {
		t.Fatalf("/api/list returned %d: %v", status, payload)
	}
	documents, ok := payload["documents"].([]any)
	if !ok {
		t.Fatalf("/api/list returned no documents: %v", payload)
	}
	slugs := make([]string, 0, len(documents))
	for _, entry := range documents {
		document, ok := entry.(map[string]any)
		if !ok {
			t.Fatalf("unexpected listing entry %v", entry)
		}
		slugs = append(slugs, text(document["slug"]))
	}
	return slugs
}

func contains(slugs []string, want string) bool {
	for _, slug := range slugs {
		if slug == want {
			return true
		}
	}
	return false
}

func TestListingShowsOnlyYourOwnUploads(t *testing.T) {
	server, instance := anyPublisherServer(t)

	mine := publishAs(t, server.URL, "alice", "Alice Paper")
	theirs := publishAs(t, server.URL, "bob", "Bob Paper")

	// An example belongs to everyone, and a document published before
	// ownership was recorded belongs to no one in particular.
	if _, err := instance.store.put("example-doc", "Example", digestOf("<p>e</p>"), "<p>e</p>", "", ""); err != nil {
		t.Fatal(err)
	}
	instance.store.mu.Lock()
	entry := instance.store.entries["example-doc"]
	entry.Example = true
	instance.store.entries["example-doc"] = entry
	instance.store.mu.Unlock()
	if _, err := instance.store.put("legacy-doc", "Legacy", digestOf("<p>l</p>"), "<p>l</p>", "", ""); err != nil {
		t.Fatal(err)
	}

	visible := slugsVisibleTo(t, server.URL, "alice")
	if !contains(visible, mine) {
		t.Fatalf("@alice should see her own document, got %v", visible)
	}
	if contains(visible, theirs) {
		t.Fatalf("@alice should not see @bob's document, got %v", visible)
	}
	if !contains(visible, "example-doc") || !contains(visible, "legacy-doc") {
		t.Fatalf("examples and unowned documents stay shared, got %v", visible)
	}
}

// Publishing without any sign-in still belongs to the browser that did it, so
// one visitor's uploads are not listed to the next.
func TestAnonymousVisitorsDoNotSeeEachOther(t *testing.T) {
	server, instance := newTestServer(t)
	instance.publishers = parsePolicy("anyone")

	mine := publishWith(t, server.URL, visitorAs("alpha"), "First Paper")
	theirs := publishWith(t, server.URL, visitorAs("beta"), "Second Paper")

	visible := slugsVisibleWith(t, server.URL, visitorAs("alpha"))
	if !contains(visible, mine) {
		t.Fatalf("a visitor should see their own upload, got %v", visible)
	}
	if contains(visible, theirs) {
		t.Fatalf("a visitor should not see another browser's upload, got %v", visible)
	}
}

// The shell is what hands a browser its name, so the first page load carries
// the cookie every later upload is owned by.
func TestTheShellNamesANewBrowser(t *testing.T) {
	server, _ := newTestServer(t)

	response, err := http.Get(server.URL + "/")
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	for _, cookie := range response.Cookies() {
		if cookie.Name == visitorCookie && cookie.Value != "" && cookie.HttpOnly {
			return
		}
	}
	t.Fatalf("the index page set no visitor cookie: %v", response.Cookies())
}

// Deleting another browser's document is a not-found, exactly as it is for
// another GitHub account's.
func TestAnonymousVisitorCannotDeleteAnothersDocument(t *testing.T) {
	server, instance := newTestServer(t)
	instance.publishers = parsePolicy("anyone")

	theirs := publishWith(t, server.URL, visitorAs("beta"), "Second Paper")
	status, payload := postAs(t, visitorAs("alpha"), server.URL,
		"/api/documents/"+theirs+"/delete", nil)
	if status != 404 {
		t.Fatalf("deleting another browser's document returned %d: %v", status, payload)
	}
	if _, ok := instance.store.get(theirs); !ok {
		t.Fatalf("another browser's document was deleted")
	}
}

// The CLI publishing to a deployment open to everyone carries no cookie and
// no token, so its uploads belong to nobody and stay shared.
func TestUploadsWithNoIdentityStayShared(t *testing.T) {
	server, instance := newTestServer(t)
	instance.publishers = parsePolicy("anyone")

	slug := publishWith(t, server.URL, "", "Command Line Paper")
	if entry, ok := instance.store.get(slug); !ok || entry.Publisher != "" {
		t.Fatalf("an unidentified upload should own nothing: %+v", entry)
	}
	if visible := slugsVisibleWith(t, server.URL, visitorAs("alpha")); !contains(visible, slug) {
		t.Fatalf("unowned documents stay shared, got %v", visible)
	}
}

func TestAnotherPublishersSlugIsNotReplaced(t *testing.T) {
	server, instance := anyPublisherServer(t)

	theirs := publishAs(t, server.URL, "bob", "Bob Paper")
	// Guessing the slug is the whole attack: an upload aimed straight at it
	// becomes a new document of @alice's instead of a replacement.
	mine := publishAs(t, server.URL, "alice", "Alice Paper", theirs)

	if mine == theirs {
		t.Fatalf("@alice took over @bob's slug %q", theirs)
	}
	entry, ok := instance.store.get(theirs)
	if !ok || entry.Publisher != "bob" {
		t.Fatalf("@bob's document changed hands: %+v", entry)
	}
	if body, err := instance.store.read(theirs, entry.SHA); err != nil || string(body) != "<p>Bob Paper</p>" {
		t.Fatalf("@bob's bytes were overwritten: %q %v", body, err)
	}
}

func TestYourOwnSlugIsStillReplacedInPlace(t *testing.T) {
	server, _ := anyPublisherServer(t)

	first := publishAs(t, server.URL, "alice", "Alice Paper")
	again := publishAs(t, server.URL, "alice", "Alice Paper", first)
	if first != again {
		t.Fatalf("republishing should keep the URL, got %q then %q", first, again)
	}
}

func TestDeletingAnotherPublishersDocumentIsANotFound(t *testing.T) {
	server, instance := anyPublisherServer(t)

	theirs := publishAs(t, server.URL, "bob", "Bob Paper")
	status, payload := postAs(t, sessionAs("alice"), server.URL,
		"/api/documents/"+theirs+"/delete", nil)
	if status != 404 {
		t.Fatalf("@alice deleting @bob's document returned %d: %v", status, payload)
	}
	if _, ok := instance.store.get(theirs); !ok {
		t.Fatalf("@bob's document was deleted by @alice")
	}

	status, payload = postAs(t, sessionAs("bob"), server.URL,
		"/api/documents/"+theirs+"/delete", nil)
	if status != 200 {
		t.Fatalf("@bob deleting his own document returned %d: %v", status, payload)
	}
}
