package main

import (
	"crypto/rand"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
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
	Example   bool   `json:"example,omitempty"`
	// Publisher is the lowercased GitHub login that uploaded this version, and
	// the only account that may replace or delete it. Empty on a reserved
	// example, and on anything published before ownership was recorded or on a
	// deployment where publishing needs no account at all.
	Publisher string `json:"publisher,omitempty"`
	// PublisherID is the GitHub account's numeric id, set alongside Publisher
	// on every new upload from a signed-in caller. A login can be renamed; the
	// numeric id cannot, so ownedBy prefers it when a document carries one.
	// Never set for a visitor-owned or unowned document, since neither has a
	// GitHub account behind it.
	PublisherID string `json:"publisher_id,omitempty"`
	// Size is the bytes of the stored HTML, and what the storage quotas below
	// are measured against. An entry from before Size was recorded reads back
	// as zero, which admit treats as free rather than refusing every upload
	// until each old document is replaced.
	Size int64 `json:"size"`
}

// ownedBy answers whether a caller -- named by ownerKey (see server.owner)
// and, when signed in, their GitHub numeric id -- may replace or delete this
// document. An entry with no publisher belongs to no one in particular and
// stays shared, which is how every document behaved before ownership was
// recorded. An entry carrying a PublisherID compares against the id instead
// of the key, since the id survives a GitHub account being renamed and the
// key would not; a legacy entry, or one owned by a visitor: key, has no
// PublisherID and falls back to comparing the key, exactly as before.
func (e indexEntry) ownedBy(ownerKey, callerID string) bool {
	switch {
	case e.Publisher == "":
		return true
	case e.PublisherID != "":
		return callerID != "" && callerID == e.PublisherID
	default:
		return e.Publisher == strings.ToLower(ownerKey)
	}
}

type store struct {
	dir string

	mu      sync.Mutex
	entries map[string]indexEntry
}

func newStore(dir string) *store {
	current := &store{dir: dir}
	entries, err := loadIndex(current.indexPath())
	if err != nil {
		die("%v", err)
	}
	current.entries = entries
	return current
}

// loadIndex reads index.json. No index yet is an empty store, which is how a
// fresh directory starts. An index that exists but cannot be read or parsed is
// a different thing entirely, and an error rather than an empty map: carrying
// on would present every stored document as gone, and the next publish would
// overwrite the real index with a near-empty one.
func loadIndex(path string) (map[string]indexEntry, error) {
	entries := map[string]indexEntry{}
	raw, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return entries, nil
	}
	if err != nil {
		return nil, fmt.Errorf("could not read %s: %w", path, err)
	}
	if err := json.Unmarshal(raw, &entries); err != nil {
		return nil, fmt.Errorf("%s is not readable as an index (%w); "+
			"move it aside to start empty", path, err)
	}
	return entries, nil
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
// The quota check and the index mutation happen under the same lock, so two
// uploads racing for the last of a quota cannot both be admitted. ownerID is
// the caller's GitHub numeric id, empty for a visitor or an unidentified
// caller.
func (s *store) put(slug, title, digest, html, owner, ownerID string) (indexEntry, error) {
	size := int64(len(html))
	// The lock is held across the file write as well as the index update. A
	// refused upload must cost nothing on disk, so admission comes first; and
	// two uploads racing for the last of a quota cannot both be admitted.
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := s.admit(slug, owner, size, time.Now()); err != nil {
		return indexEntry{}, err
	}
	if err := os.MkdirAll(s.documentDir(slug), 0o755); err != nil {
		return indexEntry{}, err
	}
	path := filepath.Join(s.documentDir(slug), digest+".html")
	if err := os.WriteFile(path, []byte(html), 0o644); err != nil {
		return indexEntry{}, err
	}
	now := timestamp()
	created := now
	example := false
	replaced := ""
	if existing, ok := s.entries[slug]; ok {
		created = existing.CreatedAt
		// A replacement keeps what the document already is: an example stays
		// an example, and its publisher -- and publisher id -- do not change
		// hands.
		example = existing.Example
		if existing.Publisher != "" {
			owner, ownerID = existing.Publisher, existing.PublisherID
		}
		replaced = existing.SHA
	}
	entry := indexEntry{
		Slug: slug, Title: title, SHA: digest, Size: size,
		CreatedAt: created, UpdatedAt: now,
		Example:     example,
		Publisher:   strings.ToLower(owner),
		PublisherID: ownerID,
	}
	previous, existed := s.entries[slug]
	s.entries[slug] = entry
	if err := s.saveLocked(); err != nil {
		// The bytes are on disk but the index naming them is not, so the
		// document does not exist as far as any later run is concerned. Undo
		// the in-memory half rather than report a success that will vanish.
		if existed {
			s.entries[slug] = previous
		} else {
			delete(s.entries, slug)
		}
		return indexEntry{}, err
	}
	// The reader only ever loads the entry's own digest, so a version this
	// replacement left behind is unreachable the moment the index above is
	// durable. Pruning it after that point, rather than before, means a
	// crash mid-write never leaves the current version missing.
	if replaced != "" && replaced != digest {
		s.pruneOtherVersions(slug, digest)
	}
	return entry, nil
}

// quotaError is what admit returns when a storage rule refuses an upload. It
// carries the HTTP status and message the rule names, so the handler answers
// with exactly what the rule decided rather than translating it a second
// time; the Worker uses the same wording.
type quotaError struct {
	status  int
	message string
}

func (e *quotaError) Error() string { return e.message }

// admit enforces the storage ceilings a write must clear, under the lock that
// makes its index entry. owner is the caller's owner key exactly as
// server.owner returns it -- a GitHub login, a visitor key, or "" for the one
// bucket every unidentified caller shares. An entry with no Size counts as
// zero, which is how a document stored before Size was recorded is treated:
// as free, not as a reason to refuse everything until it is replaced.
func (s *store) admit(slug, owner string, size int64, now time.Time) error {
	existing, replacing := s.entries[slug]
	previousSize := int64(0)
	if replacing {
		previousSize = existing.Size
	}

	var totalBytes, ownerBytes int64
	ownerDocuments := 0
	ownerUploadsThisHour := 0
	cutoff := now.Add(-time.Hour)
	for key, entry := range s.entries {
		totalBytes += entry.Size
		if entry.Publisher != owner {
			continue
		}
		ownerBytes += entry.Size
		// The document being replaced is not a new document, and is not
		// counted again against the count it already counts toward.
		if key != slug {
			ownerDocuments++
		}
		if updated, err := time.Parse(time.RFC3339, entry.UpdatedAt); err == nil && updated.After(cutoff) {
			ownerUploadsThisHour++
		}
	}
	totalBytes += size - previousSize
	ownerBytes += size - previousSize

	switch {
	case totalBytes > config.Storage.Total:
		return &quotaError{http.StatusInsufficientStorage, "this deployment has no room left"}
	case ownerBytes > config.Storage.PerOwner:
		return &quotaError{http.StatusInsufficientStorage, "your storage quota is used up; delete a document first"}
	case !replacing && ownerDocuments >= config.Storage.DocumentsPerOwner:
		return &quotaError{http.StatusInsufficientStorage, "you have reached the document limit; delete one first"}
	case ownerUploadsThisHour >= config.Storage.UploadsPerHour:
		return &quotaError{http.StatusTooManyRequests, "too many uploads this hour; try later"}
	}
	return nil
}

// pruneOtherVersions removes every stored version of slug except keep. A
// failure to remove one is not an upload failure; it leaves an unreachable
// file behind for a future cleanup to find, nothing more.
func (s *store) pruneOtherVersions(slug, keep string) {
	files, err := os.ReadDir(s.documentDir(slug))
	if err != nil {
		return
	}
	for _, file := range files {
		name := file.Name()
		if name == keep+".html" || !strings.HasSuffix(name, ".html") {
			continue
		}
		_ = os.Remove(filepath.Join(s.documentDir(slug), name))
	}
}

func (s *store) read(slug, digest string) ([]byte, error) {
	return os.ReadFile(filepath.Join(s.documentDir(slug), digest+".html"))
}

// remove deletes every stored version of a document and its index entry,
// returning how many versions went. The index entry goes last: until it does
// the document is still listed, which is a better half-state than a listing
// pointing at nothing.
func (s *store) remove(slug string) (int, error) {
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
	return removed, s.saveLocked()
}

func (s *store) saveLocked() error {
	raw, err := json.Marshal(s.entries)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(s.dir, 0o755); err != nil {
		return err
	}
	temporary := s.indexPath() + ".tmp"
	if err := os.WriteFile(temporary, raw, 0o644); err != nil {
		return err
	}
	return os.Rename(temporary, s.indexPath())
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
