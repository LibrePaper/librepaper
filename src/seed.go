package main

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"html"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// Seeding fills an empty data directory with the example documents and a
// handful of annotations on each, so there is something to look at without
// signing in and uploading by hand.
//
// It writes to the store directly rather than over HTTP: it is a development
// command, run against a directory rather than a deployment, and going through
// the API would mean holding a GitHub token to talk to your own laptop.

type seedAnnotation struct {
	Motivation string
	// Exact is the passage to anchor to. It has to appear in the rendered
	// document, and it has to appear once: prefix and suffix are computed from
	// wherever it is found.
	Exact       string
	Body        string
	Replacement string
	Tags        []string
	Creator     string
	Resolved    bool
	Replies     []string
	// Region annotates part of a figure instead of a passage, given as
	// percentages of the image.
	Region *region
}

type seedDocument struct {
	File        string
	Title       string
	Annotations []seedAnnotation
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
		raw, err := os.ReadFile(document.File)
		if err != nil {
			die("could not read %s: %v\n\n  Run `make examples` first, which renders them.", document.File, err)
		}

		slug := slugify(document.Title) + "-" + randomSuffix()
		entry, err := documentStore.put(slug, document.Title, digestOf(string(raw)), string(raw))
		if err != nil {
			die("could not store %s: %v", document.File, err)
		}

		text := visibleText(string(raw))
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
	reSpace         = regexp.MustCompile(`[ \t]+`)
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
