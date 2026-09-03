package main

// Every rule both backends must agree on lives here, and only here. The
// Worker gets these values injected into its source at deploy time, in place
// of __CONFIG__; `serve` reads them directly. A limit changed here changes
// everywhere on the next build.
type configuration struct {
	MaxHTML     int      `json:"max_html"`
	MaxComments int      `json:"max_comments"`
	RatePerHour int      `json:"rate_per_hour"`
	Caps        capLimit `json:"caps"`

	// Extensions are the only file types the reader can frame and anchor
	// comments into. The upload page checks them before sending, and the
	// server checks them again.
	Extensions []string `json:"extensions"`

	// Motivations are the W3C Web Annotation motivations an annotation may
	// carry. Using the standard vocabulary rather than an invented one means an
	// exported annotation says the same thing to any tool that reads the spec.
	Motivations       []string `json:"motivations"`
	DefaultMotivation string   `json:"default_motivation"`

	// MaxTags is how many labels one annotation may carry. Tags are what make
	// a long review navigable, but a dozen on one comment is a filing system,
	// not a label.
	MaxTags int `json:"max_tags"`

	// SlugPattern is the shape of a valid slug, as a RegExp source string.
	SlugPattern string `json:"slug_pattern"`
	SlugMax     int    `json:"slug_max"`

	// Documents are unlisted, so the URL is the only way in and the slug has to
	// be unguessable. 10 characters from a 32-symbol alphabet is 50 bits, drawn
	// from a CSPRNG; look-alike characters are left out so a link survives being
	// read aloud or retyped.
	SuffixAlphabet string `json:"suffix_alphabet"`
	SuffixLength   int    `json:"suffix_length"`
}

// capLimit is the maximum length of each free-text field on an annotation.
type capLimit struct {
	Body    int `json:"body"`
	Creator int `json:"creator"`
	Exact   int `json:"exact"`
	Context int `json:"context"`
	// Replacement is the text a suggested edit proposes in place of the
	// passage it is anchored to.
	Replacement int `json:"replacement"`
	Tag         int `json:"tag"`
}

var config = configuration{
	MaxHTML:     30 * 1024 * 1024,
	MaxComments: 500,
	RatePerHour: 20,
	Extensions:  []string{".html", ".htm", ".md", ".markdown"},
	Caps: capLimit{
		Body:        5000,
		Creator:     80,
		Exact:       1000,
		Context:     64,
		Replacement: 5000,
		Tag:         24,
	},
	Motivations:       []string{"commenting", "questioning", "highlighting", "editing", "assessing"},
	DefaultMotivation: "commenting",
	MaxTags:           6,
	SlugPattern:       `^[a-z0-9]+(?:-[a-z0-9]+)*$`,
	SlugMax:           80,
	SuffixAlphabet:    "abcdefghijkmnpqrstuvwxyz23456789",
	SuffixLength:      10,
}

// allowedMotivation keeps an unknown motivation out of storage, falling back
// to the default rather than rejecting the annotation.
func allowedMotivation(value string) string {
	for _, known := range config.Motivations {
		if value == known {
			return value
		}
	}
	return config.DefaultMotivation
}
