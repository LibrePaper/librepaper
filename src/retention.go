package main

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

const defaultExpireFrom = "updated"

// parseRetention accepts Go durations and the more convenient day suffix used
// in the command-line examples (24h and 30d are equivalent).
func parseRetention(value string) (time.Duration, error) {
	value = strings.TrimSpace(strings.ToLower(value))
	if value == "" || value == "never" || value == "off" {
		return 0, nil
	}
	if strings.HasSuffix(value, "d") {
		days, err := strconv.ParseFloat(strings.TrimSuffix(value, "d"), 64)
		if err != nil || days <= 0 {
			return 0, fmt.Errorf("invalid retention %q", value)
		}
		return time.Duration(days * float64(24*time.Hour)), nil
	}
	duration, err := time.ParseDuration(value)
	if err != nil || duration <= 0 {
		return 0, fmt.Errorf("invalid retention %q", value)
	}
	return duration, nil
}

func parseExpireFrom(value string) (string, error) {
	value = strings.TrimSpace(strings.ToLower(value))
	if value == "" {
		return defaultExpireFrom, nil
	}
	if value != "created" && value != "updated" {
		return "", fmt.Errorf("--expire-from must be 'created' or 'updated', not %q", value)
	}
	return value, nil
}

func (entry indexEntry) expiryTime(from string) (time.Time, error) {
	stamp := entry.UpdatedAt
	if from == "created" {
		stamp = entry.CreatedAt
	}
	return time.Parse(time.RFC3339, stamp)
}
