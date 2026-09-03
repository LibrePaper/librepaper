package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"html"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

// Seeding fills an empty data directory with the example documents and a
// handful of annotations on each, so there is something to look at without
// signing in and uploading by hand.
//
// It writes to the store directly rather than over HTTP: it is a development
// command, run against a directory rather than a deployment, and going through
// the API would mean holding a GitHub token to talk to your own laptop.

type seedAnnotation struct {
	Motivation string `json:"motivation"`
	// Exact is the passage to anchor to. It has to appear in the rendered
	// document, and it has to appear once: prefix and suffix are computed from
	// wherever it is found.
	Exact       string   `json:"exact"`
	Body        string   `json:"body"`
	Replacement string   `json:"replacement,omitempty"`
	Tags        []string `json:"tags,omitempty"`
	Creator     string   `json:"creator"`
	Resolved    bool     `json:"resolved,omitempty"`
	Replies     []string `json:"replies,omitempty"`
	// Region annotates part of a figure instead of a passage, given as
	// percentages of the image.
	Region *region `json:"region,omitempty"`
}

type seedDocument struct {
	File        string
	Title       string
	Annotations []seedAnnotation
}

// readSeedDocument returns the HTML a seed document is published as. A markdown
// example goes through the same renderer the publish and upload paths use, so
// what gets seeded is what Komodoc itself would have made of the file rather
// than a copy rendered by something else.
func readSeedDocument(document seedDocument) string {
	raw, err := os.ReadFile(document.File)
	if err != nil {
		die("could not read %s: %v\n\n  Run `make examples` first, which renders them.", document.File, err)
	}
	if !isMarkdown(document.File) {
		return string(raw)
	}
	rendered, err := renderMarkdownDocument(string(raw), document.Title)
	if err != nil {
		die("could not render %s: %v", document.File, err)
	}
	return rendered
}

func seed(dir string, documents []seedDocument) {
	absolute, err := filepath.Abs(dir)
	if err != nil {
		die("bad --data directory: %v", err)
	}
	// Start from nothing: seeding is for looking at the result, not for adding
	// to whatever was there.
	if err := os.RemoveAll(absolute); err != nil {
		die("could not clear %s: %v", absolute, err)
	}
	if err := os.MkdirAll(filepath.Join(absolute, "comments"), 0o755); err != nil {
		die("could not create %s: %v", absolute, err)
	}

	documentStore := newStore(absolute)
	rooms := newRoomSet(filepath.Join(absolute, "comments"))

	for _, document := range documents {
		raw := readSeedDocument(document)

		slug := slugify(document.Title) + "-" + randomSuffix()
		entry, err := documentStore.put(slug, document.Title, digestOf(raw), raw, "")
		if err != nil {
			die("could not store %s: %v", document.File, err)
		}

		text := visibleText(raw)
		room := rooms.get(slug)
		placed, missed := seedAnnotations(room, document.Annotations, text)

		fmt.Printf("  %-28s %s\n", entry.Slug, document.Title)
		fmt.Printf("      %d annotation(s)", placed)
		if missed > 0 {
			// Worth saying out loud: a phrase that is not in the rendered
			// document anchors nowhere, and the seed is meant to look right.
			fmt.Printf(", %d could not be anchored", missed)
		}
		fmt.Println()
	}
}

// seedRemote gives a deployed sandbox the same curated titles and annotations
// as the local seed. It deliberately replaces everything already there: a
// seed is a known demonstration state, not an additive publishing operation.
func seedRemote(endpointFlag string, documents []seedDocument) {
	endpoint := endpointFrom(endpointFlag)
	token := storedToken()
	status, raw := do("GET", endpoint+"/api/me", nil, nil, 30*time.Second)
	var capabilities map[string]any
	if status == 200 {
		_ = json.Unmarshal(raw, &capabilities)
	}
	examplesEnabled, _ := capabilities["examples_enabled"].(bool)

	status, listing := postAuthed(endpoint+"/api/list", map[string]any{}, token, 60*time.Second)
	if status != 200 {
		die("could not list the sandbox before seeding (%d): %v", status, detailOf(listing))
	}
	// Local seeding retains its historical replace-everything behavior. An
	// examples-enabled sandbox overwrites only its four reserved examples, so
	// deploying it cannot erase users' short-lived notebooks.
	if !examplesEnabled {
		existing, _ := listing["documents"].([]any)
		for _, value := range existing {
			document, ok := value.(map[string]any)
			if !ok {
				continue
			}
			slug := text(document["slug"])
			status, result := postAuthed(endpoint+"/api/documents/"+slug+"/delete",
				map[string]any{}, token, 120*time.Second)
			if status != 200 {
				die("could not remove %s before seeding (%d): %v", slug, status, detailOf(result))
			}
		}
	}

	fmt.Printf("seeding %s\n", endpoint)
	for _, document := range documents {
		raw := readSeedDocument(document)
		status, uploaded := postAuthed(endpoint+"/api/documents", map[string]any{
			"title":       document.Title,
			"html":        raw,
			"slug":        slugify(document.Title),
			"example":     true,
			"annotations": document.Annotations,
		}, token, 300*time.Second)
		if status != 201 {
			die("could not seed %s (%d): %v", document.File, status, detailOf(uploaded))
		}

		slug := text(uploaded["slug"])
		placed, missed := 0, 0
		if marked, _ := uploaded["example"].(bool); marked {
			// The Worker stored these as the canonical state and creates each
			// visitor's room from them on first use.
			for _, item := range document.Annotations {
				if item.Region == nil && !strings.Contains(visibleText(raw), item.Exact) {
					missed++
				} else {
					placed++
				}
			}
		} else {
			// The local server has no special example rooms; seed its ordinary
			// shared room as before.
			placed, missed = seedRemoteAnnotations(endpoint, token, slug, document.Annotations, visibleText(raw))
		}
		fmt.Printf("  %-28s %s\n", slug, document.Title)
		fmt.Printf("      %d annotation(s)", placed)
		if missed > 0 {
			fmt.Printf(", %d could not be anchored", missed)
		}
		fmt.Println()
	}
}

func seedRemoteAnnotations(endpoint, token, slug string, annotations []seedAnnotation, visible string) (placed, missed int) {
	url := endpoint + "/api/documents/" + slug + "/comments"
	for _, item := range annotations {
		at := -1
		if item.Region == nil {
			at = strings.Index(visible, item.Exact)
			if at < 0 {
				missed++
				continue
			}
		}

		incoming := message{
			Type:        "comment",
			Motivation:  item.Motivation,
			Body:        item.Body,
			Replacement: item.Replacement,
			Tags:        item.Tags,
			Creator:     item.Creator,
			Exact:       item.Exact,
			Region:      item.Region,
		}
		if at >= 0 {
			incoming.Prefix = tail(visible[:at], config.Caps.Context)
			incoming.Suffix = head(visible[at+len(item.Exact):], config.Caps.Context)
			position := at
			incoming.Position = &position
		}

		status, result := postAuthed(url, incoming, token, 60*time.Second)
		if status != 200 {
			die("could not seed an annotation on %s (%d): %v", slug, status, detailOf(result))
		}
		comment, _ := result["comment"].(map[string]any)
		commentID := text(comment["id"])

		for _, body := range item.Replies {
			status, reply := postAuthed(url, message{
				Type: "reply", CommentID: commentID, Body: body, Creator: "Reviewer",
			}, token, 60*time.Second)
			if status != 200 {
				die("could not seed a reply on %s (%d): %v", slug, status, detailOf(reply))
			}
		}
		if item.Resolved {
			status, resolved := postAuthed(url, message{
				Type: "resolve", CommentID: commentID, Resolved: true,
			}, token, 60*time.Second)
			if status != 200 {
				die("could not resolve a seeded annotation on %s (%d): %v", slug, status, detailOf(resolved))
			}
		}
		placed++
	}
	return placed, missed
}

// seedAnnotations writes one document's annotations, anchoring each to where
// its passage actually appears.
func seedAnnotations(room *room, annotations []seedAnnotation, text string) (placed, missed int) {
	for _, item := range annotations {
		at := -1
		if item.Region == nil {
			at = strings.Index(text, item.Exact)
			if at < 0 {
				missed++
				continue
			}
		}

		written := &comment{
			ID:          newID(),
			Motivation:  item.Motivation,
			Exact:       item.Exact,
			Body:        item.Body,
			Replacement: item.Replacement,
			Tags:        item.Tags,
			Creator:     item.Creator,
			Created:     timestamp(),
			Region:      item.Region,
			Replies:     []reply{},
		}
		if at >= 0 {
			written.Prefix = tail(text[:at], config.Caps.Context)
			written.Suffix = head(text[at+len(item.Exact):], config.Caps.Context)
			position := at
			written.Position = &position
		}
		if item.Resolved {
			stamp := timestamp()
			written.Resolved, written.ResolvedAt = true, &stamp
		}
		for _, answer := range item.Replies {
			written.Replies = append(written.Replies, reply{
				ID: newID(), Body: answer, Creator: "Reviewer", Created: timestamp(),
			})
		}

		room.mu.Lock()
		room.seq++
		written.Seq = room.seq
		room.comments = append(room.comments, written)
		room.save()
		room.mu.Unlock()
		placed++
	}
	return placed, missed
}

var (
	reScriptOrStyle = regexp.MustCompile(`(?is)<(script|style)\b[^>]*>.*?</(script|style)>`)
	reTag           = regexp.MustCompile(`(?s)<[^>]*>`)
	// All whitespace, newlines included: a browser renders a line break inside
	// a paragraph as a single space, so a phrase a reader can select may be
	// wrapped across lines in the source. Collapsing only spaces and tabs left
	// those unanchorable in any hand-written HTML or markdown.
	reSpace = regexp.MustCompile(`\s+`)
)

// visibleText is what the reader would anchor against: the document with its
// markup, scripts and styles removed. An approximation of what a browser shows,
// which is enough to locate a phrase and take its surroundings.
func visibleText(document string) string {
	text := reScriptOrStyle.ReplaceAllString(document, " ")
	text = reTag.ReplaceAllString(text, "")
	return reSpace.ReplaceAllString(html.UnescapeString(text), " ")
}

func head(text string, n int) string {
	if len(text) <= n {
		return text
	}
	return text[:n]
}

func tail(text string, n int) string {
	if len(text) <= n {
		return text
	}
	return text[len(text)-n:]
}

// digestOf is the content hash a document is addressed by.
func digestOf(html string) string {
	sum := sha256.Sum256([]byte(html))
	return hex.EncodeToString(sum[:])
}
