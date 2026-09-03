package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"
)

// Export in the W3C Web Annotation Data Model. The stored fields already carry
// the spec's names, so this is a reshaping rather than a translation: each
// comment becomes an Annotation whose target is a TextQuoteSelector, and each
// reply an Annotation motivated by replying.

const annotationContext = "http://www.w3.org/ns/anno.jsonld"

type annotationBody struct {
	Type    string `json:"type"`
	Value   string `json:"value"`
	Format  string `json:"format,omitempty"`
	Purpose string `json:"purpose,omitempty"`
}

// bodiesFor builds the body of an annotation: the remark itself, the labels on
// it as tagging bodies, and for a suggested edit the text it proposes. A
// single body stays a single object rather than a list of one, which is what
// the spec's examples look like.
func bodiesFor(item comment) any {
	bodies := []annotationBody{}
	if item.Body != "" {
		bodies = append(bodies, annotationBody{
			Type: "TextualBody", Value: item.Body, Format: "text/plain",
		})
	}
	if item.Replacement != "" {
		bodies = append(bodies, annotationBody{
			Type: "TextualBody", Value: item.Replacement, Format: "text/plain",
			Purpose: "editing",
		})
	}
	for _, tag := range item.Tags {
		bodies = append(bodies, annotationBody{
			Type: "TextualBody", Value: tag, Purpose: "tagging",
		})
	}
	switch len(bodies) {
	case 0:
		// A highlight has nothing to say; the spec allows a body-less
		// annotation, and the target is the point of it.
		return nil
	case 1:
		return bodies[0]
	default:
		return bodies
	}
}

type annotationAgent struct {
	Type string `json:"type"`
	Name string `json:"name"`
}

type quoteSelector struct {
	Type   string `json:"type"`
	Exact  string `json:"exact"`
	Prefix string `json:"prefix,omitempty"`
	Suffix string `json:"suffix,omitempty"`
}

type annotationTarget struct {
	Source   string `json:"source"`
	Selector any    `json:"selector,omitempty"`
}

// fragmentSelector points at part of a figure, using the Media Fragments
// syntax the spec names for exactly this: xywh in percentages, so it holds
// whatever size the image is displayed at.
type fragmentSelector struct {
	Type       string `json:"type"`
	ConformsTo string `json:"conformsTo"`
	Value      string `json:"value"`

	// Which image, which the spec has no vocabulary for: a document's figures
	// have no identifiers of their own. Ours, under our own prefix.
	ImageDigest string `json:"komodoc:image_digest,omitempty"`
	ImageIndex  int    `json:"komodoc:image_index"`
}

// selectorFor is a quotation for an annotation on words, and a rectangle for
// one on part of a figure.
func selectorFor(item comment) any {
	if item.Region != nil {
		return fragmentSelector{
			Type:       "FragmentSelector",
			ConformsTo: "http://www.w3.org/TR/media-frags/",
			Value: fmt.Sprintf("xywh=percent:%g,%g,%g,%g",
				item.Region.X, item.Region.Y, item.Region.Width, item.Region.Height),
			ImageDigest: item.Region.ImageDigest,
			ImageIndex:  item.Region.ImageIndex,
		}
	}
	return quoteSelector{
		Type:   "TextQuoteSelector",
		Exact:  item.Exact,
		Prefix: item.Prefix,
		Suffix: item.Suffix,
	}
}

type annotation struct {
	Context    string          `json:"@context,omitempty"`
	ID         string          `json:"id"`
	Type       string          `json:"type"`
	Motivation string          `json:"motivation"`
	Created    string          `json:"created"`
	Creator    annotationAgent `json:"creator"`
	// One body for a plain remark; several when an annotation also carries
	// tags or proposed text, each saying what it is for. That is how the spec
	// expresses a tagged or editorial annotation.
	Body   any              `json:"body,omitempty"`
	Target annotationTarget `json:"target"`

	// Outside the spec, which has no notion of a thread being settled. Extra
	// properties are permitted, and a reader that does not know them ignores
	// them.
	Resolved   bool    `json:"komodoc:resolved"`
	ResolvedAt *string `json:"komodoc:resolved_at,omitempty"`
}

type annotationPage struct {
	Context string       `json:"@context"`
	Type    string       `json:"type"`
	Source  string       `json:"source"`
	Label   string       `json:"label"`
	Total   int          `json:"total"`
	Items   []annotation `json:"items"`
}

func exportDocument(slug, endpointFlag, format, out string) {
	endpoint := endpointFrom(endpointFlag)

	status, raw := do("GET", endpoint+"/api/documents/"+slug, nil, nil, 30*time.Second)
	if status != 200 {
		die("no document with the slug %q at %s", slug, endpoint)
	}
	var document indexEntry
	if err := json.Unmarshal(raw, &document); err != nil {
		die("could not read the document: %v", err)
	}

	status, raw = do("GET", endpoint+"/api/documents/"+slug+"/comments", nil, nil, 60*time.Second)
	if status != 200 {
		die("could not read the comments (%d)", status)
	}
	var listing struct {
		Comments []comment `json:"comments"`
	}
	if err := json.Unmarshal(raw, &listing); err != nil {
		die("could not read the comments: %v", err)
	}

	source := endpoint + "/docs/" + slug
	var rendered string
	switch format {
	case "jsonld", "":
		rendered = renderJSONLD(document, listing.Comments, source)
	case "markdown", "md":
		rendered = renderMarkdown(document, listing.Comments, source)
	default:
		die("unknown format %q; use jsonld or markdown", format)
	}

	if out == "" || out == "-" {
		fmt.Print(rendered)
		return
	}
	if err := os.WriteFile(out, []byte(rendered), 0o644); err != nil {
		die("could not write %s: %v", out, err)
	}
	fmt.Fprintf(os.Stderr, "wrote %s (%d annotation(s))\n", out, len(listing.Comments))
}

func renderJSONLD(document indexEntry, comments []comment, source string) string {
	items := make([]annotation, 0, len(comments))
	for _, item := range comments {
		motivation := item.Motivation
		if motivation == "" {
			motivation = config.DefaultMotivation
		}
		items = append(items, annotation{
			ID:         "urn:uuid:" + item.ID,
			Type:       "Annotation",
			Motivation: motivation,
			Created:    item.Created,
			Creator:    annotationAgent{Type: "Person", Name: item.Creator},
			Body:       bodiesFor(item),
			Target:     annotationTarget{Source: source, Selector: selectorFor(item)},
			Resolved:   item.Resolved,
			ResolvedAt: item.ResolvedAt,
		})
		// A reply is an annotation whose target is the annotation it answers.
		for _, answer := range item.Replies {
			items = append(items, annotation{
				ID:         "urn:uuid:" + answer.ID,
				Type:       "Annotation",
				Motivation: "replying",
				Created:    answer.Created,
				Creator:    annotationAgent{Type: "Person", Name: answer.Creator},
				Body:       annotationBody{Type: "TextualBody", Value: answer.Body, Format: "text/plain"},
				Target:     annotationTarget{Source: "urn:uuid:" + item.ID},
			})
		}
	}

	page := annotationPage{
		Context: annotationContext,
		Type:    "AnnotationPage",
		Source:  source,
		Label:   document.Title,
		Total:   len(items),
		Items:   items,
	}
	encoded, err := json.MarshalIndent(page, "", "  ")
	if err != nil {
		die("could not encode the annotations: %v", err)
	}
	return string(encoded) + "\n"
}

func renderMarkdown(document indexEntry, comments []comment, source string) string {
	var out strings.Builder
	fmt.Fprintf(&out, "# %s\n\n%s\n\n%d annotation(s)\n", document.Title, source, len(comments))

	for _, item := range comments {
		state := ""
		if item.Resolved {
			state = " (resolved)"
		}
		motivation := item.Motivation
		if motivation == "" {
			motivation = config.DefaultMotivation
		}
		fmt.Fprintf(&out, "\n---\n\n## %s by %s%s\n\n", motivation, item.Creator, state)
		if len(item.Tags) > 0 {
			fmt.Fprintf(&out, "`%s`\n\n", strings.Join(item.Tags, "` `"))
		}
		if item.Region != nil {
			fmt.Fprintf(&out, "On figure %d, at %g%%,%g%% (%g%% by %g%%)\n\n",
				item.Region.ImageIndex+1, item.Region.X, item.Region.Y,
				item.Region.Width, item.Region.Height)
		} else {
			fmt.Fprintf(&out, "> %s\n\n", strings.ReplaceAll(item.Exact, "\n", "\n> "))
		}
		if item.Replacement != "" {
			// A suggested edit reads as what it is: this, in place of that.
			fmt.Fprintf(&out, "**Suggested:**\n\n> %s\n\n",
				strings.ReplaceAll(item.Replacement, "\n", "\n> "))
		}
		if item.Body != "" {
			fmt.Fprintf(&out, "%s\n\n", item.Body)
		}
		fmt.Fprintf(&out, "*%s*\n", item.Created)
		for _, answer := range item.Replies {
			fmt.Fprintf(&out, "\n- **%s**: %s *(%s)*\n", answer.Creator, answer.Body, answer.Created)
		}
	}
	return out.String()
}
