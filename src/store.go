package main

import (
	"crypto/rand"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
)

// The document store: the index of what exists, and the bytes of each version.
// R2's counterpart. One process owns the directory, so the compare-and-swap
// the Worker needs on index.json becomes a mutex here.

type indexEntry struct {
	Slug      string `json:"slug"`
	Title     string `json:"title"`
	SHA       string `json:"sha"`
	CreatedAt string `json:"created_at"`
	UpdatedAt string `json:"updated_at"`
}

type store struct {
	dir string

	mu      sync.Mutex
	entries map[string]indexEntry
}

func newStore(dir string) *store {
	current := &store{dir: dir, entries: map[string]indexEntry{}}
	raw, err := os.ReadFile(current.indexPath())
	if err == nil {
		_ = json.Unmarshal(raw, &current.entries)
	}
	return current
}

func (s *store) indexPath() string              { return filepath.Join(s.dir, "index.json") }
func (s *store) documentDir(slug string) string { return filepath.Join(s.dir, "documents", slug) }

func (s *store) get(slug string) (indexEntry, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	entry, ok := s.entries[slug]
	return entry, ok
}

// list returns every document, newest first, as the listing endpoint wants.
func (s *store) list() []indexEntry {
	s.mu.Lock()
	defer s.mu.Unlock()
	documents := make([]indexEntry, 0, len(s.entries))
	for _, entry := range s.entries {
		documents = append(documents, entry)
	}
	sort.SliceStable(documents, func(i, j int) bool {
		return documents[i].UpdatedAt > documents[j].UpdatedAt
	})
	return documents
}

// put writes a new version and updates the index, returning the stored entry.
func (s *store) put(slug, title, digest, html string) (indexEntry, error) {
	if err := os.MkdirAll(s.documentDir(slug), 0o755); err != nil {
		return indexEntry{}, err
	}
	path := filepath.Join(s.documentDir(slug), digest+".html")
	if err := os.WriteFile(path, []byte(html), 0o644); err != nil {
		return indexEntry{}, err
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	now := timestamp()
	created := now
	if existing, ok := s.entries[slug]; ok {
		created = existing.CreatedAt
	}
	entry := indexEntry{Slug: slug, Title: title, SHA: digest, CreatedAt: created, UpdatedAt: now}
	s.entries[slug] = entry
	s.saveLocked()
	return entry, nil
}

func (s *store) read(slug, digest string) ([]byte, error) {
	return os.ReadFile(filepath.Join(s.documentDir(slug), digest+".html"))
}

// remove deletes every stored version of a document and its index entry,
// returning how many versions went. The index entry goes last: until it does
// the document is still listed, which is a better half-state than a listing
// pointing at nothing.
func (s *store) remove(slug string) int {
	removed := 0
	if files, err := os.ReadDir(s.documentDir(slug)); err == nil {
		for _, file := range files {
			if strings.HasSuffix(file.Name(), ".html") {
				if os.Remove(filepath.Join(s.documentDir(slug), file.Name())) == nil {
					removed++
				}
			}
		}
	}
	_ = os.Remove(s.documentDir(slug))

	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.entries, slug)
	s.saveLocked()
	return removed
}

func (s *store) saveLocked() {
	raw, err := json.Marshal(s.entries)
	if err != nil {
		return
	}
	if err := os.MkdirAll(s.dir, 0o755); err != nil {
		return
	}
	temporary := s.indexPath() + ".tmp"
	if err := os.WriteFile(temporary, raw, 0o644); err != nil {
		return
	}
	_ = os.Rename(temporary, s.indexPath())
}

// randomSuffix makes a new document's link unguessable, drawing from the same
// alphabet and length the Worker uses.
func randomSuffix() string {
	bytes := make([]byte, config.SuffixLength)
	if _, err := rand.Read(bytes); err != nil {
		die("no randomness available: %v", err)
	}
	alphabet := config.SuffixAlphabet
	out := make([]byte, len(bytes))
	for i, b := range bytes {
		out[i] = alphabet[int(b)%len(alphabet)]
	}
	return string(out)
}

// slugify matches the Worker's, so the same title yields the same slug on
// either backend.
func slugify(value string) string {
	lowered := strings.ToLower(value)
	var builder strings.Builder
	previousDash := false
	for _, r := range lowered {
		if (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') {
			builder.WriteRune(r)
			previousDash = false
			continue
		}
		if !previousDash {
			builder.WriteByte('-')
			previousDash = true
		}
	}
	slug := strings.Trim(builder.String(), "-")
	runes := []rune(slug)
	if len(runes) > config.SlugMax {
		slug = string(runes[:config.SlugMax])
	}
	return strings.Trim(slug, "-")
}
