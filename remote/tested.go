package main

import (
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

//go:embed tested.json
var builtinTested []byte

var testedURL = "https://raw.githubusercontent.com/felitrejos/magi/main/remote/tested.json"

const testedFresh = time.Hour

var versionPattern = regexp.MustCompile(`[0-9]+\.[0-9]+\.[0-9]+`)

type testedRange struct {
	Min    string   `json:"min"`
	Max    string   `json:"max"`
	Broken []string `json:"broken"`
}

func parseTested(data []byte) (testedRange, error) {
	var t struct {
		Claude testedRange `json:"claude"`
	}
	if err := json.Unmarshal(data, &t); err != nil {
		return t.Claude, err
	}
	for _, v := range []string{t.Claude.Min, t.Claude.Max} {
		if _, ok := parseVersion(v); !ok {
			return t.Claude, fmt.Errorf("unreadable tested version %q", v)
		}
	}
	return t.Claude, nil
}

func loadTested() (testedRange, error) {
	path, err := store.Dir("tested.json")
	if err != nil {
		return testedRange{}, err
	}
	if info, err := os.Stat(path); err != nil || time.Since(info.ModTime()) > testedFresh {
		if data, err := fetchTested(); err == nil {
			if err := store.WriteAtomic(path, data); err != nil {
				return testedRange{}, err
			}
		}
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		data = builtinTested
	} else if err != nil {
		return testedRange{}, err
	}
	return parseTested(data)
}

func fetchTested() ([]byte, error) {
	client := http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(testedURL)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("%s answered %s", testedURL, resp.Status)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, 1<<16))
	if err != nil {
		return nil, err
	}
	_, err = parseTested(data)
	return data, err
}

func (r testedRange) refuse(where, version string) error {
	v, ok := parseVersion(version)
	low, _ := parseVersion(r.Min)
	high, _ := parseVersion(r.Max)
	switch {
	case !ok:
		return fmt.Errorf("couldn't read the Claude Code version on %s, so magi can't tell whether it was tested", where)
	case compareVersions(v, low) < 0:
		return fmt.Errorf("%s has Claude Code %s, older than magi supports; update it to %s or newer", where, version, r.Min)
	case slices.Contains(r.Broken, version):
		return fmt.Errorf("%s has Claude Code %s, which fails magi's tests, so magi doesn't move sessions with it; update it", where, version)
	case compareVersions(v, high) > 0:
		return fmt.Errorf("%s has Claude Code %s, which hasn't passed magi's nightly tests yet (tested up to %s); a new version usually passes within a day, so try again later", where, version, r.Max)
	}
	return nil
}

func refuseUntested(machines ...sh.Machine) error {
	tested, err := loadTested()
	if err != nil {
		return fmt.Errorf("reading which versions magi was tested with failed: %w", err)
	}
	for _, m := range machines {
		out, err := sh.Run(m, `"$1" --version`, m.Claude())
		if err != nil {
			return fmt.Errorf("reading the Claude Code version on %s failed: %w", m.Name(), err)
		}
		if err := tested.refuse(m.Name(), versionPattern.FindString(string(out))); err != nil {
			return err
		}
	}
	return nil
}

func parseVersion(text string) ([3]int, bool) {
	var v [3]int
	words := strings.Fields(text)
	if len(words) == 0 {
		return v, false
	}
	parts := strings.SplitN(words[0], ".", 3)
	if len(parts) != 3 {
		return v, false
	}
	for i, p := range parts {
		n, err := strconv.Atoi(p)
		if err != nil {
			return v, false
		}
		v[i] = n
	}
	return v, true
}

func compareVersions(a, b [3]int) int {
	for i := range a {
		if a[i] != b[i] {
			return a[i] - b[i]
		}
	}
	return 0
}
