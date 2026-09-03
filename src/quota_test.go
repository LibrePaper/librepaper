package main

import (
	"net/http"
	"path/filepath"
	"strings"
	"testing"
)

// The storage rules are what keep a deployment's bill bounded no matter who
// shows up. Each test below overrides one config.Storage field, low enough to
// trip on a couple of small uploads, and restores it afterward so later tests
// see the real defaults again.

func withStorage(t *testing.T, override storageLimit) {
	t.Helper()
	original := config.Storage
	config.Storage = override
	t.Cleanup(func() { config.Storage = original })
}

func TestPerOwnerByteQuotaIsRefused(t *testing.T) {
	withStorage(t, storageLimit{
		Total: 1 << 30, PerOwner: 20, DocumentsPerOwner: 50, UploadsPerHour: 30,
	})
	server, _ := newTestServer(t)

	status, payload := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Too Big", "html": strings.Repeat("x", 21),
	})
	if status != http.StatusInsufficientStorage ||
		payload["error"] != "your storage quota is used up; delete a document first" {
		t.Fatalf("got %d %v, want 507 with the per-owner quota message", status, payload)
	}
}

func TestDocumentCountLimitIsRefused(t *testing.T) {
	withStorage(t, storageLimit{
		Total: 1 << 30, PerOwner: 1 << 20, DocumentsPerOwner: 1, UploadsPerHour: 30,
	})
	server, _ := newTestServer(t)

	status, first := post(t, server.URL, "/api/documents", map[string]string{
		"title": "First", "html": "<p>first</p>",
	})
	if status != http.StatusCreated {
		t.Fatalf("first upload got %d: %v", status, first)
	}

	status, payload := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Second", "html": "<p>second</p>",
	})
	if status != http.StatusInsufficientStorage ||
		payload["error"] != "you have reached the document limit; delete one first" {
		t.Fatalf("got %d %v, want 507 with the document-limit message", status, payload)
	}
}

func TestUploadsPerHourIsRefused(t *testing.T) {
	withStorage(t, storageLimit{
		Total: 1 << 30, PerOwner: 1 << 20, DocumentsPerOwner: 50, UploadsPerHour: 1,
	})
	server, _ := newTestServer(t)

	status, first := post(t, server.URL, "/api/documents", map[string]string{
		"title": "First", "html": "<p>first</p>",
	})
	if status != http.StatusCreated {
		t.Fatalf("first upload got %d: %v", status, first)
	}

	// A second, distinct document, so this trips the hourly cap rather than
	// the document count -- which is left generous above precisely so this
	// test isolates the rule it means to check.
	status, payload := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Second", "html": "<p>second</p>",
	})
	if status != http.StatusTooManyRequests ||
		payload["error"] != "too many uploads this hour; try later" {
		t.Fatalf("got %d %v, want 429 with the rate message", status, payload)
	}
}

func TestGlobalTotalQuotaIsRefused(t *testing.T) {
	withStorage(t, storageLimit{
		Total: 10, PerOwner: 1 << 20, DocumentsPerOwner: 50, UploadsPerHour: 30,
	})
	server, _ := newTestServer(t)

	status, payload := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Too Big", "html": strings.Repeat("x", 11),
	})
	if status != http.StatusInsufficientStorage ||
		payload["error"] != "this deployment has no room left" {
		t.Fatalf("got %d %v, want 507 with the deployment-full message", status, payload)
	}
}

// Replacing a document is not a new document, and its old bytes are not still
// held once the new ones land: a quota exactly the size of one document
// should admit any number of republishes of it.
func TestReplacingDoesNotDoubleCountSize(t *testing.T) {
	withStorage(t, storageLimit{
		Total: 1 << 30, PerOwner: 30, DocumentsPerOwner: 50, UploadsPerHour: 30,
	})
	server, _ := newTestServer(t)

	html := strings.Repeat("x", 25)
	status, first := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Doc", "html": html,
	})
	if status != http.StatusCreated {
		t.Fatalf("first upload got %d: %v", status, first)
	}
	slug := text(first["slug"])

	status, second := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Doc", "slug": slug, "html": html,
	})
	if status != http.StatusCreated {
		t.Fatalf("replacing the same document should not double-count its size, got %d: %v",
			status, second)
	}
}

// A replacement leaves the old version unreachable, so its bytes are removed
// rather than kept forever alongside the one the index actually names.
func TestReplacingRemovesTheSupersededVersion(t *testing.T) {
	server, instance := newTestServer(t)

	status, first := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Doc", "html": "<p>version one</p>",
	})
	if status != http.StatusCreated {
		t.Fatalf("first upload got %d: %v", status, first)
	}
	slug, firstSHA := text(first["slug"]), text(first["sha"])

	status, second := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Doc", "slug": slug, "html": "<p>version two</p>",
	})
	if status != http.StatusCreated {
		t.Fatalf("second upload got %d: %v", status, second)
	}
	secondSHA := text(second["sha"])
	if secondSHA == firstSHA {
		t.Fatal("a changed document should get a new digest")
	}

	if _, err := instance.store.read(slug, firstSHA); err == nil {
		t.Fatal("the superseded version should have been removed from disk")
	}
	body, err := instance.store.read(slug, secondSHA)
	if err != nil || !strings.Contains(string(body), "version two") {
		t.Fatalf("the current version should still be readable, got %q, %v", body, err)
	}
}

// The listing is how a publisher sees what is eating their quota, so the
// bytes recorded for a document need to be visible there.
func TestListIncludesDocumentSize(t *testing.T) {
	server, _ := newTestServer(t)

	html := "<p>" + strings.Repeat("y", 40) + "</p>"
	status, uploaded := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Sized Doc", "html": html,
	})
	if status != http.StatusCreated {
		t.Fatalf("upload got %d: %v", status, uploaded)
	}

	status, payload := post(t, server.URL, "/api/list", map[string]any{})
	if status != http.StatusOK {
		t.Fatalf("listing returned %d: %v", status, payload)
	}
	documents, _ := payload["documents"].([]any)
	if len(documents) != 1 {
		t.Fatalf("expected one listed document, got %v", documents)
	}
	entry, _ := documents[0].(map[string]any)
	size, ok := entry["size"].(float64)
	if !ok || int(size) != len(html) {
		t.Fatalf("listed size was %v, want %d", entry["size"], len(html))
	}
}

// A refused upload must leave nothing behind: an over-quota publisher who
// keeps trying would otherwise fill the disk with orphaned versions.
func TestRefusedUploadWritesNothing(t *testing.T) {
	withStorage(t, storageLimit{Total: 1 << 30, PerOwner: 10, DocumentsPerOwner: 50, UploadsPerHour: 50})
	server, instance := newTestServer(t)
	status, _ := post(t, server.URL, "/api/documents", map[string]string{
		"title": "Too Big", "html": "<p>this is more than ten bytes</p>",
	})
	if status != http.StatusInsufficientStorage {
		t.Fatalf("want 507, got %d", status)
	}
	files, _ := filepath.Glob(filepath.Join(instance.store.dir, "documents", "*", "*.html"))
	if len(files) != 0 {
		t.Fatalf("a refused upload left files behind: %v", files)
	}
}
