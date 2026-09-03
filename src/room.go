package main

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

// A room owns one document's comments and the open sockets of everyone reading
// it, so a write by one reader reaches the others without anybody polling.
// This is the Durable Object's counterpart: one process, so the mutex plays the
// part the platform's single-threaded actor plays on Cloudflare.

// Field names follow the W3C Web Annotation Data Model, so exporting is a
// reshaping rather than a translation: exact, prefix and suffix are a
// TextQuoteSelector, motivation is the standard vocabulary, and creator and
// created mean what the spec says. resolved is ours; the spec has no notion of
// it, and permits extra properties.

type reply struct {
	ID      string `json:"id"`
	Body    string `json:"body"`
	Creator string `json:"creator"`
	Created string `json:"created"`
}

// A rectangle on an image, in percentages of the image's own size, so it
// survives the document being displayed at any width.
//
// Which image is a harder question than where on it. There is no text around a
// figure to anchor to, so two identifiers are kept: a digest of the image
// source, which survives the figure moving, and its position among the
// document's images, which survives the image being re-encoded. The reader
// tries the digest first.
type region struct {
	ImageDigest string  `json:"image_digest"`
	ImageIndex  int     `json:"image_index"`
	X           float64 `json:"x"`
	Y           float64 `json:"y"`
	Width       float64 `json:"w"`
	Height      float64 `json:"h"`
}

type comment struct {
	ID         string `json:"id"`
	Seq        int    `json:"seq"`
	Motivation string `json:"motivation"`
	Exact      string `json:"exact"`
	Prefix     string `json:"prefix"`
	Suffix     string `json:"suffix"`
	// Where the passage sat when the comment was made. A pointer so a comment
	// written before it was recorded stays null rather than claiming offset 0.
	Position *int `json:"position"`
	// Set instead of the text selector when the annotation is on part of a
	// figure rather than on a run of words.
	Region *region `json:"region,omitempty"`
	Body   string  `json:"body"`
	// Replacement is what a suggested edit proposes in place of the passage.
	Replacement string   `json:"replacement,omitempty"`
	Tags        []string `json:"tags,omitempty"`
	Creator     string   `json:"creator"`
	Created     string   `json:"created"`
	Resolved    bool     `json:"resolved"`
	ResolvedAt  *string  `json:"resolved_at"`
	Replies     []reply  `json:"replies"`
}

// roomState is what lands on disk, one file per document.
type roomState struct {
	Seq      int        `json:"seq"`
	Comments []*comment `json:"comments"`
}

type room struct {
	slug string
	path string

	mu       sync.Mutex
	seq      int
	comments []*comment
	sockets  map[*wsConn]string // socket to client address
	rate     map[string]int     // "address:hour" to count
}

type roomSet struct {
	mu    sync.Mutex
	dir   string
	rooms map[string]*room
}

func newRoomSet(dir string) *roomSet {
	return &roomSet{dir: dir, rooms: map[string]*room{}}
}

func (set *roomSet) get(slug string) *room {
	set.mu.Lock()
	defer set.mu.Unlock()
	if existing, ok := set.rooms[slug]; ok {
		return existing
	}
	current := &room{
		slug:    slug,
		path:    filepath.Join(set.dir, slug+".json"),
		sockets: map[*wsConn]string{},
		rate:    map[string]int{},
	}
	current.load()
	set.rooms[slug] = current
	return current
}

// purge drops a document's comments and disconnects anyone still reading it.
// Reached only through the delete route, which checks the password first.
func (set *roomSet) purge(slug string) {
	current := set.get(slug)

	current.mu.Lock()
	current.comments = nil
	current.seq = 0
	sockets := make([]*wsConn, 0, len(current.sockets))
	for socket := range current.sockets {
		sockets = append(sockets, socket)
	}
	current.mu.Unlock()

	_ = os.Remove(current.path)
	for _, socket := range sockets {
		socket.close(1000, "document deleted")
	}

	set.mu.Lock()
	delete(set.rooms, slug)
	set.mu.Unlock()
}

func (r *room) load() {
	raw, err := os.ReadFile(r.path)
	if err != nil {
		return
	}
	var state roomState
	if err := json.Unmarshal(raw, &state); err != nil {
		return
	}
	r.seq, r.comments = state.Seq, state.Comments
	sort.SliceStable(r.comments, func(i, j int) bool { return r.comments[i].Seq < r.comments[j].Seq })
}

// save rewrites the whole file. Comment volume per document is in the dozens,
// so this stays cheaper than any incremental scheme, and a rename makes it
// atomic.
func (r *room) save() error {
	state := roomState{Seq: r.seq, Comments: r.comments}
	if len(state.Comments) == 0 {
		state.Comments = []*comment{}
	}
	raw, err := json.Marshal(state)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(r.path), 0o755); err != nil {
		return err
	}
	temporary := r.path + ".tmp"
	if err := os.WriteFile(temporary, raw, 0o644); err != nil {
		return err
	}
	return os.Rename(temporary, r.path)
}

func (r *room) snapshot() []*comment {
	r.mu.Lock()
	defer r.mu.Unlock()
	if len(r.comments) == 0 {
		return []*comment{}
	}
	return append([]*comment{}, r.comments...)
}

func (r *room) counts() (int, int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	open := 0
	for _, item := range r.comments {
		if !item.Resolved {
			open++
		}
	}
	return len(r.comments), open
}

func (r *room) attach(socket *wsConn, address string) {
	r.mu.Lock()
	r.sockets[socket] = address
	r.mu.Unlock()
}

func (r *room) detach(socket *wsConn) {
	r.mu.Lock()
	delete(r.sockets, socket)
	r.mu.Unlock()
}

func (r *room) broadcast(payload any) {
	message, err := json.Marshal(payload)
	if err != nil {
		return
	}
	r.mu.Lock()
	sockets := make([]*wsConn, 0, len(r.sockets))
	for socket := range r.sockets {
		sockets = append(sockets, socket)
	}
	r.mu.Unlock()

	for _, socket := range sockets {
		// A failing socket is on its way out; the read loop cleans it up.
		_ = socket.writeText(message)
	}
}

// rateOk counts writes per address per hour, and forgets older hours as it
// goes rather than accumulating an entry per address per hour.
func (r *room) rateOk(address string) bool {
	if address == "" {
		return true
	}
	hour := time.Now().Unix() / 3600
	key := fmt.Sprintf("%s:%d", address, hour)
	suffix := fmt.Sprintf(":%d", hour)
	for existing := range r.rate {
		if !strings.HasSuffix(existing, suffix) {
			delete(r.rate, existing)
		}
	}
	if r.rate[key]+1 > config.RatePerHour {
		return false
	}
	r.rate[key]++
	return true
}

// message is one client frame. Every field is optional; apply decides which
// ones a given type needs.
type message struct {
	Type        string   `json:"type"`
	Motivation  string   `json:"motivation"`
	Body        string   `json:"body"`
	Replacement string   `json:"replacement"`
	Tags        []string `json:"tags"`
	Creator     string   `json:"creator"`
	Exact       string   `json:"exact"`
	Prefix      string   `json:"prefix"`
	Suffix      string   `json:"suffix"`
	Position    *int     `json:"position"`
	Region      *region  `json:"region"`
	CommentID   string   `json:"comment_id"`
	Resolved    bool     `json:"resolved"`
	TempID      string   `json:"temp_id"`
}

// apply validates, persists, and returns the event to broadcast. The second
// result is false when the event is an error, which goes only to its sender.
func (r *room) apply(incoming message, address string) (map[string]any, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()

	fail := func(text string) (map[string]any, bool) {
		return map[string]any{"type": "error", "message": text, "temp_id": incoming.TempID}, false
	}
	// Every change here is acknowledged to its sender and broadcast to every
	// other reader, so it has to be on disk first. When the write fails the
	// in-memory change is undone, keeping this process and the file agreeing
	// with each other rather than diverging until the next restart.
	unsaved := func(undo func()) (map[string]any, bool) {
		undo()
		return fail("could not save that comment; try again")
	}

	if incoming.Type == "resolve" {
		target := r.find(incoming.CommentID)
		if target == nil {
			return fail("unknown comment")
		}
		wasResolved, wasResolvedAt := target.Resolved, target.ResolvedAt
		target.Resolved = incoming.Resolved
		if target.Resolved {
			stamp := timestamp()
			target.ResolvedAt = &stamp
		} else {
			target.ResolvedAt = nil
		}
		if r.save() != nil {
			return unsaved(func() { target.Resolved, target.ResolvedAt = wasResolved, wasResolvedAt })
		}
		return map[string]any{
			"type":        "resolve",
			"comment_id":  target.ID,
			"resolved":    target.Resolved,
			"resolved_at": target.ResolvedAt,
		}, true
	}

	if incoming.Type == "delete" {
		for index, item := range r.comments {
			if item.ID == incoming.CommentID {
				kept := append([]*comment{}, r.comments...)
				r.comments = append(r.comments[:index:index], r.comments[index+1:]...)
				if r.save() != nil {
					return unsaved(func() { r.comments = kept })
				}
				return map[string]any{"type": "delete", "comment_id": incoming.CommentID}, true
			}
		}
		return fail("unknown comment")
	}

	if !r.rateOk(address) {
		return fail("too many comments from this address; try later")
	}

	body := strings.TrimSpace(clean(incoming.Body, config.Caps.Body))
	motivation := allowedMotivation(incoming.Motivation)
	// A highlight is the passage itself: marking something as worth returning
	// to needs no words. Everything else is a remark, and a remark with no
	// words is nothing.
	if body == "" && !(incoming.Type == "comment" && motivation == "highlighting") {
		return fail("comment body is required")
	}
	creator := strings.TrimSpace(clean(incoming.Creator, config.Caps.Creator))
	if creator == "" {
		creator = "Anonymous"
	}

	switch incoming.Type {
	case "reply":
		target := r.find(incoming.CommentID)
		if target == nil {
			return fail("unknown comment")
		}
		added := reply{ID: newID(), Body: body, Creator: creator, Created: timestamp()}
		target.Replies = append(target.Replies, added)
		if r.save() != nil {
			return unsaved(func() { target.Replies = target.Replies[:len(target.Replies)-1] })
		}
		return map[string]any{
			"type":       "reply",
			"comment_id": target.ID,
			"reply":      added,
			"temp_id":    incoming.TempID,
		}, true

	case "comment":
		if len(r.comments) >= config.MaxComments {
			return fail("this document has reached its comment limit")
		}
		exact := strings.TrimSpace(clean(incoming.Exact, config.Caps.Exact))
		spot := validRegion(incoming.Region)
		// An annotation is anchored to words or to part of a figure; one or
		// the other, never neither.
		if exact == "" && spot == nil {
			return fail("select some text or part of a figure to comment on")
		}
		r.seq++
		// The selector is the durable anchor. Offsets are recomputed in the
		// reader against whatever version of the document is on screen, so
		// replacing a document needs no migration pass here.
		added := &comment{
			ID:         newID(),
			Seq:        r.seq,
			Motivation: motivation,
			Exact:      exact,
			Prefix:     clean(incoming.Prefix, config.Caps.Context),
			Suffix:     clean(incoming.Suffix, config.Caps.Context),
			Position:   nonNegative(incoming.Position),
			Region:     spot,
			Body:       body,
			// Only a suggested edit proposes replacement text; anything else
			// sending it is ignored rather than refused.
			Replacement: replacementFor(motivation, incoming.Replacement),
			Tags:        cleanTags(incoming.Tags),
			Creator:     creator,
			Created:     timestamp(),
			Replies:     []reply{},
		}
		r.comments = append(r.comments, added)
		if r.save() != nil {
			return unsaved(func() {
				r.comments = r.comments[:len(r.comments)-1]
				r.seq--
			})
		}
		return map[string]any{"type": "comment", "comment": added, "temp_id": incoming.TempID}, true
	}

	return fail("unknown message type")
}

func (r *room) find(id string) *comment {
	for _, item := range r.comments {
		if item.ID == id {
			return item
		}
	}
	return nil
}

func timestamp() string {
	return time.Now().UTC().Format("2006-01-02T15:04:05Z")
}

// clean strips control characters and trims to a length, matching what the
// Worker stores.
func clean(value string, limit int) string {
	filtered := strings.Map(func(r rune) rune {
		if r < 0x09 || (r >= 0x0b && r <= 0x0c) || (r >= 0x0e && r <= 0x1f) || r == 0x7f {
			return -1
		}
		return r
	}, value)
	runes := []rune(filtered)
	if len(runes) > limit {
		return string(runes[:limit])
	}
	return string(runes)
}

// newID returns a random UUID v4, as crypto.randomUUID does in the Worker.
func newID() string {
	var bytes [16]byte
	if _, err := rand.Read(bytes[:]); err != nil {
		die("no randomness available: %v", err)
	}
	bytes[6] = (bytes[6] & 0x0f) | 0x40
	bytes[8] = (bytes[8] & 0x3f) | 0x80
	encoded := hex.EncodeToString(bytes[:])
	return encoded[0:8] + "-" + encoded[8:12] + "-" + encoded[12:16] + "-" + encoded[16:20] + "-" + encoded[20:]
}

// nonNegative keeps a position hint only when the client sent a sensible one.
func nonNegative(value *int) *int {
	if value == nil || *value < 0 {
		return nil
	}
	return value
}

// replacementFor keeps proposed text only where it means something.
func replacementFor(motivation, replacement string) string {
	if motivation != "editing" {
		return ""
	}
	return strings.TrimSpace(clean(replacement, config.Caps.Replacement))
}

// cleanTags normalises labels: lowercased, trimmed, deduplicated, capped in
// both length and number, so filtering by one of them is predictable.
func cleanTags(tags []string) []string {
	out := []string{}
	seen := map[string]bool{}
	for _, tag := range tags {
		label := strings.ToLower(strings.TrimSpace(clean(tag, config.Caps.Tag)))
		label = strings.Join(strings.Fields(label), " ")
		if label == "" || seen[label] {
			continue
		}
		seen[label] = true
		out = append(out, label)
		if len(out) == config.MaxTags {
			break
		}
	}
	return out
}

// validRegion keeps a rectangle only if it is one: inside the image, with a
// size worth drawing. Percentages, so it holds at any display width.
func validRegion(spot *region) *region {
	if spot == nil {
		return nil
	}
	inside := func(v float64) bool { return v >= 0 && v <= 100 }
	if !inside(spot.X) || !inside(spot.Y) || !inside(spot.Width) || !inside(spot.Height) {
		return nil
	}
	if spot.Width < 0.5 || spot.Height < 0.5 || spot.X+spot.Width > 100.5 || spot.Y+spot.Height > 100.5 {
		return nil
	}
	if spot.ImageIndex < 0 {
		return nil
	}
	return &region{
		ImageDigest: clean(spot.ImageDigest, 64),
		ImageIndex:  spot.ImageIndex,
		X:           spot.X, Y: spot.Y, Width: spot.Width, Height: spot.Height,
	}
}
