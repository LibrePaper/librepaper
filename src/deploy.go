package main

import (
	"encoding/json"
	"fmt"
	"os"
)

const compatibilityDate = "2025-08-01"

func ensureBucket(cf *cloudflare) {
	body, _ := json.Marshal(map[string]string{"name": bucket})
	status, raw := cf.raw("POST", "/accounts/"+cf.account+"/r2/buckets",
		map[string]string{"content-type": "application/json"}, body)

	if status == 200 || status == 201 {
		fmt.Printf("  created R2 bucket %s\n", bucket)
		return
	}
	codes := errorCodes(raw)
	// 10004 is "bucket already exists", which is the normal case on redeploy.
	if codes[10004] || status == 409 {
		fmt.Printf("  R2 bucket %s already exists\n", bucket)
		return
	}
	// 10042 means the account has never switched R2 on. It is a one-time click
	// in the dashboard, and the raw API error does not say where to go.
	if codes[10042] {
		die("R2 is not enabled on this account yet.\n\n" +
			"  Open https://dash.cloudflare.com, choose R2 in the left sidebar,\n" +
			"  and enable it. Cloudflare may ask for a payment method; the free\n" +
			"  tier this uses (10 GB stored, no egress charge) still costs nothing.\n\n" +
			"  Then run deploy again.")
	}
	if status == 401 || status == 403 {
		die("not allowed to create the bucket (%d). Check the API token has\n"+
			"  Account / Workers R2 Storage / Edit.\n\n  %s",
			status, truncate(string(raw), 200))
	}
	die("could not create the bucket (%d): %s", status, truncate(string(raw), 300))
}

// ensureSubdomain returns the account workers.dev subdomain, which is the
// second label of every URL this deploys. It is set once in the dashboard,
// applies to every Worker on the account, and is not this tool's to change.
// Check it before uploading, so a missing one costs nothing.
func ensureSubdomain(cf *cloudflare) string {
	status, raw := cf.raw("GET", "/accounts/"+cf.account+"/workers/subdomain", nil, nil)
	if status == 200 {
		var payload struct {
			Result struct {
				Subdomain string `json:"subdomain"`
			} `json:"result"`
		}
		if err := json.Unmarshal(raw, &payload); err == nil && payload.Result.Subdomain != "" {
			return payload.Result.Subdomain
		}
	}
	die("this account has no workers.dev subdomain yet, and a Worker cannot be\n" +
		"  uploaded without one.\n\n" +
		"  Open https://dash.cloudflare.com, go to Workers & Pages, and set one\n" +
		"  in the right-hand sidebar of the settings page. It is account-wide,\n" +
		"  so pick something that suits everything you might host there.\n\n" +
		"  Then run deploy again.")
	return ""
}

type deployOptions struct {
	label        string
	clientID     string
	clientSecret string
	publishers   string
	commenters   string
	expireAfter  string
	expireFrom   string
	examples     bool
}

func deploy(options deployOptions) {
	expireAfterValue := firstOf(options.expireAfter, os.Getenv("KOMODOC_EXPIRE_AFTER"))
	retention, err := parseRetention(expireAfterValue)
	if err != nil {
		die("%v; use a duration such as 24h or 30d", err)
	}
	expireFromValue := firstOf(options.expireFrom, os.Getenv("KOMODOC_EXPIRE_FROM"))
	expireFrom, err := parseExpireFrom(expireFromValue)
	if err != nil {
		die("%v", err)
	}
	configure(options.label)
	token := os.Getenv("CLOUDFLARE_API_TOKEN")
	if token == "" {
		die("set CLOUDFLARE_API_TOKEN (see --help for the permissions it needs)")
	}
	cf := newCloudflare(token, os.Getenv("CLOUDFLARE_ACCOUNT_ID"))
	fmt.Printf("account %s\n", cf.account)

	ensureBucket(cf)
	subdomain := ensureSubdomain(cf)
	existing := cf.scriptExists()

	// Identity is GitHub's. The client id and the policies are plain settings;
	// the client secret and the cookie-signing key are secrets, and a redeploy
	// inherits both rather than asking again.
	clientID := firstOf(options.clientID, os.Getenv("KOMODOC_GITHUB_CLIENT_ID"))
	if clientID == "" && !existing {
		die("this needs a GitHub OAuth app.\n\n"+
			"  Create one at https://github.com/settings/developers, with the\n"+
			"  callback URL https://%s.%s.workers.dev/auth/callback, then pass\n"+
			"  --client-id and --client-secret, or set:\n\n"+
			"    export KOMODOC_GITHUB_CLIENT_ID=...\n"+
			"    export KOMODOC_GITHUB_CLIENT_SECRET=...", scriptName, subdomain)
	}

	publishers := parsePublishPolicy(firstOf(options.publishers, os.Getenv("KOMODOC_PUBLISHERS")))
	if len(publishers.Logins) == 0 && !publishers.Any && !existing {
		die("say who may publish, with --publishers.\n\n" +
			"    --publishers your-github-login      only you\n" +
			"    --publishers alice,bob              those accounts\n" +
			"    --publishers any                    any GitHub account")
	}
	commenters := parsePolicy(firstOf(options.commenters, os.Getenv("KOMODOC_COMMENTERS"), "anyone"))

	bindings := []any{
		map[string]string{"type": "r2_bucket", "name": "DOCS", "bucket_name": bucket},
		map[string]string{"type": "durable_object_namespace", "name": "ROOM", "class_name": "Room"},
	}
	bindings = append(bindings, settingBinding(cf, existing, "KOMODOC_GITHUB_CLIENT_ID", clientID))
	bindings = append(bindings, settingBinding(cf, existing, "KOMODOC_PUBLISHERS", publishers.String()))
	bindings = append(bindings, settingBinding(cf, existing, "KOMODOC_COMMENTERS", commenters.String()))
	bindings = append(bindings, map[string]string{"type": "plain_text", "name": "KOMODOC_EXAMPLES", "text": fmt.Sprint(options.examples)})
	if existing && expireAfterValue == "" {
		bindings = append(bindings, map[string]string{"type": "inherit", "name": "KOMODOC_EXPIRE_SECONDS"})
		bindings = append(bindings, map[string]string{"type": "inherit", "name": "KOMODOC_EXPIRE_FROM"})
	} else {
		bindings = append(bindings, map[string]string{"type": "plain_text", "name": "KOMODOC_EXPIRE_SECONDS", "text": fmt.Sprint(int64(retention.Seconds()))})
		bindings = append(bindings, map[string]string{"type": "plain_text", "name": "KOMODOC_EXPIRE_FROM", "text": expireFrom})
	}

	secret := firstOf(options.clientSecret, os.Getenv("KOMODOC_GITHUB_CLIENT_SECRET"))
	switch {
	case secret != "":
		bindings = append(bindings, map[string]string{
			"type": "secret_text", "name": "KOMODOC_GITHUB_CLIENT_SECRET", "text": secret})
	case existing:
		bindings = append(bindings, map[string]string{
			"type": "inherit", "name": "KOMODOC_GITHUB_CLIENT_SECRET"})
		fmt.Println("  keeping the existing GitHub client secret")
	default:
		die("set --client-secret, or $KOMODOC_GITHUB_CLIENT_SECRET")
	}

	// The key that signs session cookies. Generated once; inherited after, so
	// a redeploy does not sign everybody out.
	if existing {
		bindings = append(bindings, map[string]string{"type": "inherit", "name": "KOMODOC_SESSION_KEY"})
	} else {
		bindings = append(bindings, map[string]string{
			"type": "secret_text", "name": "KOMODOC_SESSION_KEY", "text": randomToken() + randomToken()})
	}

	fmt.Printf("  publishing: %s\n", publishers.describe())
	fmt.Printf("  commenting: %s\n", commenters.describe())

	metadata := map[string]any{
		"main_module":        "worker.js",
		"compatibility_date": compatibilityDate,
		"bindings":           bindings,
	}
	if !existing {
		// The Durable Object class is registered once, on first deploy.
		metadata["migrations"] = map[string]any{"new_tag": "v1", "new_sqlite_classes": []string{"Room"}}
	}

	source := workerSource()
	registering := ""
	if !existing {
		registering = " and registering the Room class"
	}
	fmt.Printf("  uploading worker (%d KiB)%s\n", len(source)/1024, registering)

	encoded, err := json.Marshal(metadata)
	if err != nil {
		die("could not encode the upload metadata: %v", err)
	}
	contentType, body := buildMultipart([]formPart{
		{name: "metadata", contentType: "application/json", body: string(encoded)},
		{name: "worker.js", filename: "worker.js", contentType: "application/javascript+module", body: source},
	})
	cf.call("PUT", "/accounts/"+cf.account+"/workers/scripts/"+scriptName,
		map[string]string{"content-type": contentType}, body)

	cf.callJSON("POST", "/accounts/"+cf.account+"/workers/scripts/"+scriptName+"/subdomain",
		map[string]any{"enabled": true, "previews_enabled": false})
	// With no retention option, a redeploy preserves the trigger and bindings
	// already installed. "--expire-after never" explicitly removes them.
	if expireAfterValue != "" || !existing {
		schedules := []any{}
		if retention > 0 {
			schedules = append(schedules, map[string]string{"cron": "0 * * * *"})
			fmt.Printf("  expiry: %s after %s (daily cleanup)\n", expireFrom, retention)
		}
		cf.callJSON("PUT", "/accounts/"+cf.account+"/workers/scripts/"+scriptName+"/schedules", schedules)
	}

	// The same bundle again, under <name>-docs, which is where documents are
	// served from. It needs the bucket and nothing else: no Durable Object, no
	// secrets, no session. A document served there is a stranger to the reader,
	// which is what lets it run its own scripts safely.
	docsName := scriptName + "-docs"
	fmt.Printf("  uploading document origin (%s)\n", docsName)
	docsMetadata, err := json.Marshal(map[string]any{
		"main_module":        "worker.js",
		"compatibility_date": compatibilityDate,
		"bindings": []any{
			map[string]string{"type": "r2_bucket", "name": "DOCS", "bucket_name": bucket},
		},
	})
	if err != nil {
		die("could not encode the upload metadata: %v", err)
	}
	docsType, docsBody := buildMultipart([]formPart{
		{name: "metadata", contentType: "application/json", body: string(docsMetadata)},
		{name: "worker.js", filename: "worker.js", contentType: "application/javascript+module", body: source},
	})
	cf.call("PUT", "/accounts/"+cf.account+"/workers/scripts/"+docsName,
		map[string]string{"content-type": docsType}, docsBody)
	cf.callJSON("POST", "/accounts/"+cf.account+"/workers/scripts/"+docsName+"/subdomain",
		map[string]any{"enabled": true, "previews_enabled": false})

	endpoint := fmt.Sprintf("https://%s.%s.workers.dev", scriptName, subdomain)
	fmt.Printf("\ndeployed: %s\n", endpoint)
	fmt.Println("\nnext:")
	fmt.Printf("  export KOMODOC_ENDPOINT=%s\n", endpoint)
	fmt.Printf("  %s publish paper.html --title 'My Paper'\n", os.Args[0])
	if !existing {
		fmt.Println("\nThe first request after a deploy can take a second while it warms up.")
	}
}

// settingBinding writes a plain setting, or keeps the deployed one when this
// run was not told what it should be.
func settingBinding(cf *cloudflare, existing bool, name, value string) map[string]string {
	if value == "" && existing {
		return map[string]string{"type": "inherit", "name": name}
	}
	return map[string]string{"type": "plain_text", "name": name, "text": value}
}
