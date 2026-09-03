package main

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"reflect"
	"regexp"
	"testing"
)

// The two backends enforce the same rules in two languages, which is the one
// piece of duplication the design cannot remove: the Worker runs where Go does
// not. What it can do is stop the two drifting apart silently. conformance.json
// holds the inputs and the answers both are expected to give, and this file
// runs each implementation over them -- Go directly, the Worker under node,
// against the very script `deploy` uploads.

type conformance struct {
	Policies []struct {
		Value    string          `json:"value"`
		Describe string          `json:"describe"`
		Allows   map[string]bool `json:"allows"`
	} `json:"policies"`
	Slugs []struct {
		Value string `json:"value"`
		Valid bool   `json:"valid"`
	} `json:"slugs"`
	Motivations []struct {
		Value string `json:"value"`
		Kept  string `json:"kept"`
	} `json:"motivations"`
	Tags []struct {
		Value []string `json:"value"`
		Kept  []string `json:"kept"`
	} `json:"tags"`
	Regions []struct {
		Value *region `json:"value"`
		Kept  bool    `json:"kept"`
	} `json:"regions"`
	Messages []struct {
		Value string `json:"value"`
		Limit int    `json:"limit"`
		Kept  string `json:"kept"`
	} `json:"messages"`
}

func loadConformance(t *testing.T) conformance {
	t.Helper()
	raw, err := os.ReadFile("conformance.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures conformance
	if err := json.Unmarshal(raw, &fixtures); err != nil {
		t.Fatal(err)
	}
	return fixtures
}

func TestGoMatchesTheConformanceFixtures(t *testing.T) {
	fixtures := loadConformance(t)

	for _, item := range fixtures.Policies {
		parsed := parsePolicy(item.Value)
		if got := parsed.describe(); got != item.Describe {
			t.Errorf("describe(%q) = %q, want %q", item.Value, got, item.Describe)
		}
		for login, want := range item.Allows {
			if got := parsed.allows(login); got != want {
				t.Errorf("policy %q allows(%q) = %v, want %v", item.Value, login, got, want)
			}
		}
	}
	for _, item := range fixtures.Slugs {
		if got := reSlug.MatchString(item.Value); got != item.Valid {
			t.Errorf("slug %q valid = %v, want %v", item.Value, got, item.Valid)
		}
	}
	for _, item := range fixtures.Motivations {
		if got := allowedMotivation(item.Value); got != item.Kept {
			t.Errorf("motivation %q = %q, want %q", item.Value, got, item.Kept)
		}
	}
	for _, item := range fixtures.Tags {
		if got := cleanTags(item.Value); !reflect.DeepEqual(got, item.Kept) {
			t.Errorf("tags %q = %q, want %q", item.Value, got, item.Kept)
		}
	}
	for _, item := range fixtures.Regions {
		if got := validRegion(item.Value) != nil; got != item.Kept {
			t.Errorf("region %+v kept = %v, want %v", item.Value, got, item.Kept)
		}
	}
	for _, item := range fixtures.Messages {
		if got := clean(item.Value, item.Limit); got != item.Kept {
			t.Errorf("clean(%q, %d) = %q, want %q", item.Value, item.Limit, got, item.Kept)
		}
	}
}

// The Worker's answers, gathered by running the built script under node with an
// epilogue that applies it to the fixtures. Nothing is stubbed: this is the
// same source workerSource() hands to Cloudflare, so a rule that drifts in
// either language shows up here as a mismatch.
const conformanceHarness = `
const fixtures = JSON.parse(process.argv[process.argv.length - 1]);
void DurableObject;
const answers = {
  policies: fixtures.policies.map((item) => {
    const parsed = parsePolicy(item.value);
    const allows = {};
    for (const login of Object.keys(item.allows)) allows[login] = policyAllows(parsed, login);
    return { describe: describePolicy(parsed), allows };
  }),
  slugs: fixtures.slugs.map((item) => SLUG.test(item.value)),
  motivations: fixtures.motivations.map((item) =>
    MOTIVATIONS.includes(item.value) ? item.value : CONFIG.default_motivation),
  tags: fixtures.tags.map((item) => cleanTags(item.value)),
  regions: fixtures.regions.map((item) => validRegion(item.value) !== null),
  messages: fixtures.messages.map((item) => clean(item.value, item.limit)),
};
console.log(JSON.stringify(answers));
`

type workerAnswers struct {
	Policies []struct {
		Describe string          `json:"describe"`
		Allows   map[string]bool `json:"allows"`
	} `json:"policies"`
	Slugs       []bool     `json:"slugs"`
	Motivations []string   `json:"motivations"`
	Tags        [][]string `json:"tags"`
	Regions     []bool     `json:"regions"`
	Messages    []string   `json:"messages"`
}

func TestWorkerMatchesTheConformanceFixtures(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("node is not installed; the Worker half of conformance is unchecked")
	}
	fixtures := loadConformance(t)
	raw, err := os.ReadFile("conformance.json")
	if err != nil {
		t.Fatal(err)
	}

	// The one thing node cannot supply is the Cloudflare runtime the Room class
	// extends, so that import becomes a local stub. Every rule under test is a
	// plain function and untouched by the substitution.
	script := reCloudflareImport.ReplaceAllString(workerSource(),
		"class DurableObject { constructor(state, env) { this.ctx = state; this.env = env; } }\n") +
		conformanceHarness
	command := exec.Command(node, "--input-type=module", "-", string(raw))
	command.Stdin = bytes.NewReader([]byte(script))
	var out, errors bytes.Buffer
	command.Stdout, command.Stderr = &out, &errors
	if err := command.Run(); err != nil {
		t.Fatalf("running the worker under node: %v\n%s", err, errors.String())
	}

	var answers workerAnswers
	if err := json.Unmarshal(out.Bytes(), &answers); err != nil {
		t.Fatalf("decoding the worker's answers: %v\n%s", err, out.String())
	}

	for i, item := range fixtures.Policies {
		if answers.Policies[i].Describe != item.Describe {
			t.Errorf("worker describe(%q) = %q, want %q",
				item.Value, answers.Policies[i].Describe, item.Describe)
		}
		for login, want := range item.Allows {
			if got := answers.Policies[i].Allows[login]; got != want {
				t.Errorf("worker policy %q allows(%q) = %v, want %v", item.Value, login, got, want)
			}
		}
	}
	for i, item := range fixtures.Slugs {
		if answers.Slugs[i] != item.Valid {
			t.Errorf("worker slug %q valid = %v, want %v", item.Value, answers.Slugs[i], item.Valid)
		}
	}
	for i, item := range fixtures.Motivations {
		if answers.Motivations[i] != item.Kept {
			t.Errorf("worker motivation %q = %q, want %q", item.Value, answers.Motivations[i], item.Kept)
		}
	}
	for i, item := range fixtures.Tags {
		if !reflect.DeepEqual(answers.Tags[i], item.Kept) {
			t.Errorf("worker tags %q = %q, want %q", item.Value, answers.Tags[i], item.Kept)
		}
	}
	for i, item := range fixtures.Regions {
		if answers.Regions[i] != item.Kept {
			t.Errorf("worker region %+v kept = %v, want %v", item.Value, answers.Regions[i], item.Kept)
		}
	}
	for i, item := range fixtures.Messages {
		if answers.Messages[i] != item.Kept {
			t.Errorf("worker clean(%q, %d) = %q, want %q",
				item.Value, item.Limit, answers.Messages[i], item.Kept)
		}
	}
}

// The Room class extends Cloudflare's DurableObject, imported from a scheme
// only their runtime resolves.
var reCloudflareImport = regexp.MustCompile(`(?m)^import \{ DurableObject \} from "cloudflare:workers";\n`)
