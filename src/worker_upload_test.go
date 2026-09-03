package main

import (
	"bytes"
	"encoding/json"
	"os/exec"
	"testing"
)

// worker_upload_test.go drives handleUpload's storage rules -- size, quotas,
// and stale-version cleanup -- under node, against an in-memory stub of the
// R2 binding. It runs the very script deploy uploads, the way
// conformance_test.go does, rather than a reimplementation of its rules.
const uploadHarness = `
const store = new Map();
let etagSeq = 0;
const env = {
  KOMODOC_PUBLISHERS: "anyone",
  KOMODOC_EXAMPLES: "alice",
  DOCS: {
    async get(key) {
      const obj = store.get(key);
      return obj ? { etag: obj.etag, json: async () => JSON.parse(obj.body), text: async () => obj.body } : null;
    },
    async put(key, body, opts) {
      const current = store.get(key);
      if (opts && opts.onlyIf) {
        const currentEtag = current ? current.etag : null;
        if (opts.onlyIf.etagMatches && opts.onlyIf.etagMatches !== currentEtag) return null;
        if (opts.onlyIf.etagDoesNotMatch === "*" && currentEtag) return null;
      }
      const etag = "e" + (++etagSeq);
      store.set(key, { body, etag });
      return { etag };
    },
    async delete(keys) { for (const k of Array.isArray(keys) ? keys : [keys]) store.delete(k); },
    async list({ prefix }) {
      return { objects: [...store.keys()].filter((k) => k.startsWith(prefix)).map((key) => ({ key })), truncated: false };
    },
  },
  ROOM: { idFromName: (n) => n, get: () => ({ fetch: async () => new Response("{}") }) },
};

async function upload(body) {
  const req = new Request("https://x/api/documents", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
  const res = await handleUpload(req, env);
  return { status: res.status, body: await res.json() };
}

async function run() {
  const out = {};
  Object.assign(CONFIG.storage, { total: 1000000, per_owner: 1000000, documents_per_owner: 2, uploads_per_hour: 1000 });

  out.first = await upload({ title: "one", html: "x".repeat(100) });
  out.replace = await upload({ title: "one", slug: out.first.body.slug, html: "y".repeat(50) });
  out.versions = [...store.keys()].filter((k) => k.startsWith("documents/" + out.first.body.slug + "/"));
  out.second = await upload({ title: "two", html: "z".repeat(50) });
  out.third = await upload({ title: "three", html: "w".repeat(50) });

  CONFIG.storage.per_owner = 60;
  CONFIG.storage.documents_per_owner = 1000;
  out.overQuota = await upload({ title: "big", html: "b".repeat(80) });

  out.example = await upload({ title: "eg", html: "<p>e</p>", example: true, annotations: [] });

  console.log(JSON.stringify(out));
}
run();
`

func TestWorkerUploadQuotas(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("node is not installed; the Worker's quota rules are unchecked")
	}
	script := reCloudflareImport.ReplaceAllString(workerSource(),
		"class DurableObject { constructor(state, env) { this.ctx = state; this.env = env; } }\n") +
		uploadHarness
	cmd := exec.Command(node, "--input-type=module", "-")
	cmd.Stdin = bytes.NewReader([]byte(script))
	var out, errs bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errs
	if err := cmd.Run(); err != nil {
		t.Fatalf("running the worker under node: %v\n%s", err, errs.String())
	}

	var got struct {
		First struct {
			Status int
			Body   struct {
				Slug string
				Size int
			}
		}
		Replace struct {
			Status int
			Body   struct{ Size int }
		}
		Versions []string
		Second   struct{ Status int }
		Third    struct {
			Status int
			Body   struct{ Error string }
		}
		OverQuota struct {
			Status int
			Body   struct{ Error string }
		} `json:"overQuota"`
		Example struct {
			Status int
			Body   struct{ Error string }
		}
	}
	if err := json.Unmarshal(out.Bytes(), &got); err != nil {
		t.Fatalf("decoding results: %v\n%s", err, out.String())
	}

	if got.First.Status != 201 || got.First.Body.Size != 100 {
		t.Errorf("first upload = %+v, want 201 with size 100", got.First)
	}
	if got.Replace.Status != 201 || got.Replace.Body.Size != 50 {
		t.Errorf("replace = %+v, want 201 with size 50", got.Replace)
	}
	if len(got.Versions) != 1 {
		t.Errorf("versions after replace = %v, want exactly the replacement's digest", got.Versions)
	}
	if got.Second.Status != 201 {
		t.Errorf("second document = %+v, want 201", got.Second)
	}
	if got.Third.Status != 507 || got.Third.Body.Error != "you have reached the document limit; delete one first" {
		t.Errorf("third document = %+v, want 507 document-limit refusal", got.Third)
	}
	if got.OverQuota.Status != 507 || got.OverQuota.Body.Error != "your storage quota is used up; delete a document first" {
		t.Errorf("over-quota upload = %+v, want 507 per-owner refusal", got.OverQuota)
	}
	if got.Example.Status != 403 {
		t.Errorf("anonymous example upload = %+v, want 403", got.Example)
	}
}
