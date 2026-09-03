package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestSeedRemoteUsesCuratedTitlesAndAnnotations(t *testing.T) {
	http, instance := newTestServer(t)
	instance.publishers = parsePolicy("anyone")

	file := filepath.Join(t.TempDir(), "example.html")
	if err := os.WriteFile(file, []byte("<h1>Filename title</h1><p>The phrase to annotate.</p>"), 0o644); err != nil {
		t.Fatal(err)
	}
	documents := []seedDocument{{
		File:  file,
		Title: "Curated title",
		Annotations: []seedAnnotation{{
			Motivation: "commenting",
			Exact:      "The phrase to annotate.",
			Body:       "Curated comment",
			Creator:    "Reviewer",
			Resolved:   true,
			Replies:    []string{"Curated reply"},
		}},
	}}

	seedRemote(http.URL, documents)
	seedRemote(http.URL, documents)

	entries := instance.store.list()
	if len(entries) != 1 || entries[0].Title != "Curated title" {
		t.Fatalf("remote seed entries are %#v", entries)
	}
	comments := instance.rooms.get(entries[0].Slug).snapshot()
	if len(comments) != 1 || comments[0].Body != "Curated comment" {
		t.Fatalf("remote seed comments are %#v", comments)
	}
	if !comments[0].Resolved || len(comments[0].Replies) != 1 || comments[0].Replies[0].Body != "Curated reply" {
		t.Fatalf("remote seed lost annotation state: %#v", comments[0])
	}
}
