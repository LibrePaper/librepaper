package main

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

const api = "https://api.cloudflare.com/client/v4"

// envelope is the shape every Cloudflare API reply has.
type envelope struct {
	Success bool            `json:"success"`
	Errors  []apiError      `json:"errors"`
	Result  json.RawMessage `json:"result"`
}

type apiError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

type cloudflare struct {
	token   string
	account string
}

func newCloudflare(token, account string) *cloudflare {
	cf := &cloudflare{token: token, account: account}
	if cf.account == "" {
		cf.account = cf.findAccount()
	}
	return cf
}

func (cf *cloudflare) raw(method, path string, headers map[string]string, body []byte) (int, []byte) {
	merged := map[string]string{"authorization": "Bearer " + cf.token}
	for name, value := range headers {
		merged[name] = value
	}
	return do(method, api+path, merged, body, 120*time.Second)
}

// call makes a request and returns its result, exiting on any failure the API
// reports.
func (cf *cloudflare) call(method, path string, headers map[string]string, body []byte) json.RawMessage {
	status, raw := cf.raw(method, path, headers, body)
	var payload envelope
	if err := json.Unmarshal(raw, &payload); err != nil {
		die("%s %s returned %d: %s", method, path, status, truncate(string(raw), 400))
	}
	if !payload.Success {
		messages := make([]string, 0, len(payload.Errors))
		for _, item := range payload.Errors {
			messages = append(messages, fmt.Sprintf("%d: %s", item.Code, item.Message))
		}
		detail := strings.Join(messages, "; ")
		if detail == "" {
			detail = truncate(string(raw), 400)
		}
		die("%s %s failed (%d) -- %s", method, path, status, detail)
	}
	return payload.Result
}

func (cf *cloudflare) callJSON(method, path string, payload any) json.RawMessage {
	body, err := json.Marshal(payload)
	if err != nil {
		die("could not encode the request: %v", err)
	}
	return cf.call(method, path, map[string]string{"content-type": "application/json"}, body)
}

func (cf *cloudflare) findAccount() string {
	var accounts []struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}
	result := cf.call("GET", "/accounts", nil, nil)
	if err := json.Unmarshal(result, &accounts); err != nil {
		die("could not read the account list: %v", err)
	}
	if len(accounts) == 0 {
		die("this token can see no accounts; set CLOUDFLARE_ACCOUNT_ID")
	}
	if len(accounts) > 1 {
		names := make([]string, 0, len(accounts))
		for _, account := range accounts {
			names = append(names, fmt.Sprintf("%s (%s)", account.Name, account.ID))
		}
		die("several accounts visible; set CLOUDFLARE_ACCOUNT_ID to one of: %s", strings.Join(names, ", "))
	}
	return accounts[0].ID
}

func (cf *cloudflare) scriptExists() bool {
	status, _ := cf.raw("GET", "/accounts/"+cf.account+"/workers/scripts/"+scriptName, nil, nil)
	return status == 200
}

// errorCodes pulls the numeric codes out of a reply that may not have parsed.
func errorCodes(raw []byte) map[int]bool {
	codes := map[int]bool{}
	var payload envelope
	if err := json.Unmarshal(raw, &payload); err != nil {
		return codes
	}
	for _, item := range payload.Errors {
		codes[item.Code] = true
	}
	return codes
}
