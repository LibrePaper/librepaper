package main

import (
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"sort"
	"time"
)

// destroyDocument deletes one document: its versions, its listing, and its
// comments.
func destroyDocument(slug, endpointFlag string, yes bool) {
	endpoint := endpointFrom(endpointFlag)

	status, raw := do("GET", endpoint+"/api/documents/"+slug, nil, nil, 30*time.Second)
	if status != 200 {
		die("no document with the slug %q at %s", slug, endpoint)
	}
	var document map[string]any
	if err := json.Unmarshal(raw, &document); err != nil {
		die("could not read the document listing: %v", err)
	}

	count := 0
	if value, ok := document["comment_count"].(float64); ok {
		count = int(value)
	}
	fmt.Printf("About to permanently delete from %s:\n", endpoint)
	fmt.Printf("  %s  %s\n", slug, text(document["title"]))
	fmt.Printf("  %d comment(s), and every reply to them\n", count)
	fmt.Println("\nThe document, its history and its comments all go. The link stops")
	fmt.Println("working. Nothing else on this deployment is touched.")

	if !yes {
		if !isTerminal(os.Stdin) {
			die("refusing to delete without a terminal to confirm at; pass --yes if you are certain")
		}
		if confirm(fmt.Sprintf("\nType '%s' to confirm: ", slug)) != slug {
			fmt.Println("aborted, nothing was deleted")
			return
		}
	}

	status, payload := postAuthed(endpoint+"/api/documents/"+slug+"/delete",
		map[string]any{}, requireToken(), 120*time.Second)
	if status != 200 {
		die("delete failed (%d): %v", status, detailOf(payload))
	}
	versions := 0
	if value, ok := payload["versions_removed"].(float64); ok {
		versions = int(value)
	}
	fmt.Printf("\ndeleted %s (%d stored version(s))\n", text(payload["deleted"]), versions)
}

// destroyService deletes the Worker, every Durable Object, and the bucket.
func destroyService(label string, yes bool) {
	configure(label)
	token := os.Getenv("CLOUDFLARE_API_TOKEN")
	if token == "" {
		die("set CLOUDFLARE_API_TOKEN")
	}
	cf := newCloudflare(token, os.Getenv("CLOUDFLARE_ACCOUNT_ID"))

	// Show what is about to be lost, so the confirmation is informed rather
	// than reflexive.
	var documents []map[string]any
	status, raw := cf.raw("GET", "/accounts/"+cf.account+"/r2/buckets/"+bucket+"/objects/index.json", nil, nil)
	if status == 200 {
		var index map[string]map[string]any
		if err := json.Unmarshal(raw, &index); err == nil {
			for _, entry := range index {
				documents = append(documents, entry)
			}
		}
	}

	fmt.Printf("About to permanently delete, from Cloudflare account %s:\n", cf.account)
	fmt.Printf("  the Workers '%s' and '%s-docs', and the URLs they answer on\n", scriptName, scriptName)
	fmt.Println("  every Durable Object, which is where ALL COMMENTS AND REPLIES live")
	fmt.Printf("  the R2 bucket '%s', and every document in it\n", bucket)
	if len(documents) > 0 {
		sortByUpdated(documents)
		fmt.Printf("\n%d document(s) would be destroyed:\n", len(documents))
		for _, document := range documents {
			fmt.Printf("  %s  %s\n", text(document["slug"]), text(document["title"]))
		}
	}
	fmt.Println("\nThis cannot be undone. Nothing here is backed up, the comments exist")
	fmt.Println("nowhere else, and every share link you have sent out will break.")

	if !yes {
		if !isTerminal(os.Stdin) {
			die("refusing to destroy without a terminal to confirm at; pass --yes if you are certain")
		}
		if confirm("\nType 'destroy' to confirm: ") != "destroy" {
			fmt.Println("aborted, nothing was deleted")
			return
		}
	}

	// The Worker goes first: while it is live it can write new objects into the
	// bucket, and R2 will not delete a bucket that is not empty.
	for _, name := range []string{scriptName, scriptName + "-docs"} {
		status, raw = cf.raw("DELETE", "/accounts/"+cf.account+"/workers/scripts/"+name+"?force=true", nil, nil)
		switch status {
		case 200:
			fmt.Printf("  deleted worker %s\n", name)
		case 404:
			fmt.Printf("  worker %s was already gone\n", name)
		default:
			die("could not delete the worker (%d): %s", status, truncate(string(raw), 300))
		}
	}

	deleted := 0
	cursor := ""
	for {
		query := "?per_page=1000"
		if cursor != "" {
			query += "&cursor=" + url.QueryEscape(cursor)
		}
		result := cf.call("GET", "/accounts/"+cf.account+"/r2/buckets/"+bucket+"/objects"+query, nil, nil)

		keys, next := parseObjects(result)
		for _, key := range keys {
			cf.raw("DELETE", "/accounts/"+cf.account+"/r2/buckets/"+bucket+"/objects/"+url.PathEscape(key), nil, nil)
			deleted++
		}
		cursor = next
		if cursor == "" {
			break
		}
	}
	fmt.Printf("  deleted %d object(s) from %s\n", deleted, bucket)

	status, _ = cf.raw("DELETE", "/accounts/"+cf.account+"/r2/buckets/"+bucket, nil, nil)
	switch status {
	case 200, 204:
		fmt.Printf("  deleted bucket %s\n", bucket)
	case 404:
		fmt.Printf("  bucket %s was already gone\n", bucket)
	default:
		fmt.Printf("  could not delete the bucket (%d); remove it in the dashboard\n", status)
	}

	fmt.Println("\ndestroyed")
}

// parseObjects reads an R2 object listing, which comes back either as a bare
// array of objects or as an object carrying the array and a cursor.
func parseObjects(result json.RawMessage) ([]string, string) {
	var list []struct {
		Key string `json:"key"`
	}
	if err := json.Unmarshal(result, &list); err == nil {
		keys := make([]string, 0, len(list))
		for _, entry := range list {
			keys = append(keys, entry.Key)
		}
		return keys, ""
	}

	var paged struct {
		Objects []struct {
			Key string `json:"key"`
		} `json:"objects"`
		Cursor string `json:"cursor"`
	}
	if err := json.Unmarshal(result, &paged); err != nil {
		return nil, ""
	}
	keys := make([]string, 0, len(paged.Objects))
	for _, entry := range paged.Objects {
		keys = append(keys, entry.Key)
	}
	return keys, paged.Cursor
}

// sortByUpdated orders documents oldest first, as the destroy listing does.
func sortByUpdated(documents []map[string]any) {
	sort.SliceStable(documents, func(i, j int) bool {
		return text(documents[i]["updated_at"]) < text(documents[j]["updated_at"])
	})
}
