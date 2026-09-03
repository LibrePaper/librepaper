package main

import (
	"testing"
	"time"
)

func TestParseRetention(t *testing.T) {
	for input, want := range map[string]time.Duration{
		"": 0, "never": 0, "24h": 24 * time.Hour, "30d": 30 * 24 * time.Hour,
	} {
		got, err := parseRetention(input)
		if err != nil || got != want {
			t.Errorf("parseRetention(%q) = %v, %v; want %v", input, got, err, want)
		}
	}
	if _, err := parseRetention("tomorrow"); err == nil {
		t.Fatal("invalid retention was accepted")
	}
}

func TestDeleteExpired(t *testing.T) {
	dir := t.TempDir()
	s := &server{store: newStore(dir), rooms: newRoomSet(dir + "/comments")}
	s.store.entries["old"] = indexEntry{
		Slug: "old", CreatedAt: "2026-01-01T00:00:00Z", UpdatedAt: "2026-01-03T00:00:00Z",
	}
	s.store.entries["new"] = indexEntry{
		Slug: "new", CreatedAt: "2026-01-01T00:00:00Z", UpdatedAt: "2026-01-10T00:00:00Z",
	}

	removed := s.deleteExpired(time.Date(2026, 1, 11, 0, 0, 0, 0, time.UTC), 7*24*time.Hour, "updated")
	if removed != 1 {
		t.Fatalf("removed %d documents, want 1", removed)
	}
	if _, ok := s.store.get("old"); ok {
		t.Fatal("old document survived")
	}
	if _, ok := s.store.get("new"); !ok {
		t.Fatal("new document was removed")
	}
}
