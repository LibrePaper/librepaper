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

	// Storage is what keeps a deployment's bill bounded no matter who shows
	// up: a ceiling on everything stored, a ceiling per publisher, and a cap on
	// how many documents and how many uploads an hour one publisher gets.
	// Sizes are bytes of stored HTML; every index entry records its own.
	Storage storageLimit `json:"storage"`

	// MaxAnnotations caps the serialized seed annotations a reserved example
	// carries, in bytes. They are stored beside the document and re-read to
	// build every visitor's room, so they are kept small.
	MaxAnnotations int `json:"max_annotations"`

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

	// MaxTitle caps a document title, in runes. Titles live in the index, which
	// the Worker reads on nearly every request, so an unbounded title is a way
	// to sink the whole deployment.
	MaxTitle int `json:"max_title"`
	// MaxReplies caps replies on one comment, so a thread cannot grow without
	// bound and a room stays small enough to load and rewrite whole.
	MaxReplies int `json:"max_replies"`

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

// storageLimit bounds what a deployment will hold. Total and PerOwner are
// bytes; DocumentsPerOwner and UploadsPerHour are counts.
type storageLimit struct {
	Total             int64 `json:"total"`
	PerOwner          int64 `json:"per_owner"`
	DocumentsPerOwner int   `json:"documents_per_owner"`
	UploadsPerHour    int   `json:"uploads_per_hour"`
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
	MaxHTML:     4 * 1024 * 1024,
	MaxComments: 500,
	RatePerHour: 20,
	Storage: storageLimit{
		Total:             5 * 1024 * 1024 * 1024,
		PerOwner:          100 * 1024 * 1024,
		DocumentsPerOwner: 50,
		UploadsPerHour:    30,
	},
	MaxAnnotations: 256 * 1024,
	Extensions:     []string{".html", ".htm", ".md", ".markdown"},
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
	MaxTitle:          200,
	MaxReplies:        100,
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

// setMaxHTML overrides the document size ceiling, in megabytes. Both backends
// read config.MaxHTML, and the Worker and the upload page get it injected at
// deploy time, so setting it here at startup is enough to move the limit
// everywhere.
func setMaxHTML(megabytes int) {
	if megabytes == 0 {
		return
	}
	if megabytes < 1 || megabytes > 100 {
		die("--max-size must be between 1 and 100 MB")
	}
	config.MaxHTML = megabytes * 1024 * 1024
}

// setStorage overrides the storage ceilings, in megabytes: how much one
// publisher may hold across all their documents, and how much the whole
// deployment will hold. Zero leaves a default alone.
func setStorage(quotaMB, totalMB int) {
	if quotaMB < 0 || totalMB < 0 {
		die("--quota and --storage must be positive")
	}
	if quotaMB > 0 {
		config.Storage.PerOwner = int64(quotaMB) * 1024 * 1024
	}
	if totalMB > 0 {
		config.Storage.Total = int64(totalMB) * 1024 * 1024
	}
	if config.Storage.PerOwner > config.Storage.Total {
		die("--quota (%d MB) cannot exceed --storage (%d MB)",
			config.Storage.PerOwner>>20, config.Storage.Total>>20)
	}
}
