package main

import (
	"encoding/json"
	"strings"
	"testing"
)

func sampleComments() []comment {
	resolvedAt := "2026-09-02T12:00:00Z"
	return []comment{{
		ID:         "11111111-1111-4111-8111-111111111111",
		Seq:        1,
		Motivation: "questioning",
		Exact:      "the quick brown fox",
		Prefix:     "before ",
		Suffix:     " after",
		Body:       "is this right?",
		Creator:    "Vincent",
		Created:    "2026-09-02T11:00:00Z",
		Resolved:   true,
		ResolvedAt: &resolvedAt,
		Replies: []reply{{
			ID:      "22222222-2222-4222-8222-222222222222",
			Body:    "yes",
			Creator: "Reader",
			Created: "2026-09-02T11:30:00Z",
		}},
	}}
}

func TestExportIsValidWebAnnotation(t *testing.T) {
	rendered := renderJSONLD(
		indexEntry{Slug: "paper-abc", Title: "My Paper"},
		sampleComments(),
		"https://example.test/docs/paper-abc",
	)

	var page map[string]any
	if err := json.Unmarshal([]byte(rendered), &page); err != nil {
		t.Fatalf("export is not valid JSON: %v", err)
	}
	if page["@context"] != annotationContext || page["type"] != "AnnotationPage" {
		t.Fatalf("wrong envelope: %v", page)
	}
	items, _ := page["items"].([]any)
	if len(items) != 2 {
		t.Fatalf("want the comment and its reply, got %d items", len(items))
	}

	first := items[0].(map[string]any)
	if first["type"] != "Annotation" || first["motivation"] != "questioning" {
		t.Fatalf("annotation came out as %v", first)
	}
	if first["created"] != "2026-09-02T11:00:00Z" {
		t.Fatalf("created came out as %v", first["created"])
	}
	creator := first["creator"].(map[string]any)
	if creator["type"] != "Person" || creator["name"] != "Vincent" {
		t.Fatalf("creator came out as %v", creator)
	}
	body := first["body"].(map[string]any)
	if body["type"] != "TextualBody" || body["value"] != "is this right?" {
		t.Fatalf("body came out as %v", body)
	}

	target := first["target"].(map[string]any)
	if target["source"] != "https://example.test/docs/paper-abc" {
		t.Fatalf("target source came out as %v", target["source"])
	}
	selector := target["selector"].(map[string]any)
	if selector["type"] != "TextQuoteSelector" || selector["exact"] != "the quick brown fox" ||
		selector["prefix"] != "before " || selector["suffix"] != " after" {
		t.Fatalf("selector came out as %v", selector)
	}

	// Resolution state is ours, so it must not squat on a spec property name.
	if first["komodoc:resolved"] != true {
		t.Fatalf("resolved state was lost: %v", first)
	}

	// A reply is an annotation motivated by replying, targeting its parent.
	second := items[1].(map[string]any)
	if second["motivation"] != "replying" {
		t.Fatalf("reply motivation came out as %v", second["motivation"])
	}
	replyTarget := second["target"].(map[string]any)
	if replyTarget["source"] != "urn:uuid:11111111-1111-4111-8111-111111111111" {
		t.Fatalf("reply should target its parent, got %v", replyTarget["source"])
	}
	if _, hasSelector := replyTarget["selector"]; hasSelector {
		t.Fatal("a reply targets an annotation, so it needs no selector")
	}
}

func TestExportMarkdown(t *testing.T) {
	rendered := renderMarkdown(
		indexEntry{Slug: "paper-abc", Title: "My Paper"},
		sampleComments(),
		"https://example.test/docs/paper-abc",
	)
	for _, want := range []string{
		"# My Paper",
		"## questioning by Vincent (resolved)",
		"> the quick brown fox",
		"is this right?",
		"**Reader**: yes",
	} {
		if !strings.Contains(rendered, want) {
			t.Fatalf("markdown export is missing %q:\n%s", want, rendered)
		}
	}
}

func TestMotivationFallsBackToTheDefault(t *testing.T) {
	if allowedMotivation("questioning") != "questioning" {
		t.Fatal("a known motivation should be kept")
	}
	if allowedMotivation("mischief") != config.DefaultMotivation {
		t.Fatal("an unknown motivation should fall back to the default")
	}
	if allowedMotivation("") != config.DefaultMotivation {
		t.Fatal("a missing motivation should fall back to the default")
	}
}

func TestStoredMotivation(t *testing.T) {
	server, _ := newTestServer(t)
	document := publishTestDocument(t, server.URL)
	slug := document["slug"].(string)

	socket := dialWebSocket(t, server.URL, slug)
	defer socket.Close()
	readSocketMessage(t, socket)

	writeSocketMessage(t, socket, map[string]string{
		"type": "comment", "exact": "hello", "body": "why?", "motivation": "questioning",
	})
	event := readSocketMessage(t, socket)
	stored := event["comment"].(map[string]any)
	if stored["motivation"] != "questioning" {
		t.Fatalf("motivation came back as %v", stored["motivation"])
	}

	writeSocketMessage(t, socket, map[string]string{
		"type": "comment", "exact": "world", "body": "hm", "motivation": "mischief",
	})
	event = readSocketMessage(t, socket)
	stored = event["comment"].(map[string]any)
	if stored["motivation"] != config.DefaultMotivation {
		t.Fatalf("unknown motivation was stored as %v", stored["motivation"])
	}
}

func TestExportCarriesTagsAndSuggestions(t *testing.T) {
	items := []comment{{
		ID: "1", Motivation: "editing", Exact: "the quick fox", Body: "clearer",
		Replacement: "the quick brown fox", Tags: []string{"style", "typo"},
		Creator: "Vincent", Created: "2026-09-02T11:00:00Z", Replies: []reply{},
	}, {
		// A highlight says nothing, so it has no body at all.
		ID: "2", Motivation: "highlighting", Exact: "worth returning to",
		Creator: "Reader", Created: "2026-09-02T12:00:00Z", Replies: []reply{},
	}}

	var page map[string]any
	if err := json.Unmarshal([]byte(renderJSONLD(indexEntry{Title: "P"}, items, "https://x.test/d")), &page); err != nil {
		t.Fatal(err)
	}
	all := page["items"].([]any)

	// The remark, the proposed text and the two labels, each saying what it is
	// for, which is how the spec expresses this.
	bodies := all[0].(map[string]any)["body"].([]any)
	if len(bodies) != 4 {
		t.Fatalf("want four bodies, got %d: %v", len(bodies), bodies)
	}
	purposes := map[string]string{}
	for _, entry := range bodies {
		body := entry.(map[string]any)
		purposes[text(body["purpose"])] = text(body["value"])
	}
	if purposes[""] != "clearer" || purposes["editing"] != "the quick brown fox" {
		t.Fatalf("bodies came out as %v", purposes)
	}
	tagged := 0
	for _, entry := range bodies {
		if entry.(map[string]any)["purpose"] == "tagging" {
			tagged++
		}
	}
	if tagged != 2 {
		t.Fatalf("want two tagging bodies, got %d", tagged)
	}

	// A highlight is a target with nothing said about it.
	if _, has := all[1].(map[string]any)["body"]; has {
		t.Fatal("a highlight should export with no body")
	}
}

func TestExportRegionAsFragmentSelector(t *testing.T) {
	items := []comment{{
		ID: "1", Motivation: "commenting", Body: "the axis is unlabelled",
		Region:  &region{ImageDigest: "abc123", ImageIndex: 2, X: 10, Y: 20, Width: 30, Height: 25},
		Creator: "Vincent", Created: "2026-09-03T10:00:00Z", Replies: []reply{},
	}}

	var page map[string]any
	if err := json.Unmarshal([]byte(renderJSONLD(indexEntry{Title: "P"}, items, "https://x.test/d")), &page); err != nil {
		t.Fatal(err)
	}
	selector := page["items"].([]any)[0].(map[string]any)["target"].(map[string]any)["selector"].(map[string]any)

	// The spec's own way of pointing at part of an image.
	if selector["type"] != "FragmentSelector" {
		t.Fatalf("selector type was %v", selector["type"])
	}
	if selector["conformsTo"] != "http://www.w3.org/TR/media-frags/" {
		t.Fatalf("conformsTo was %v", selector["conformsTo"])
	}
	if selector["value"] != "xywh=percent:10,20,30,25" {
		t.Fatalf("value was %v", selector["value"])
	}
	// Which image has no vocabulary in the spec, so it goes under our prefix.
	if selector["komodoc:image_digest"] != "abc123" || selector["komodoc:image_index"] != float64(2) {
		t.Fatalf("image identifiers came out as %v", selector)
	}
}
