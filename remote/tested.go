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
	"time"
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

type testedVersions struct {
	Claude testedRange `json:"claude"`
	Codex  testedRange `json:"codex"`
}

func parseTested(data []byte) (testedVersions, error) {
	var t testedVersions
	if err := json.Unmarshal(data, &t); err != nil {
		return t, err
	}
	for _, r := range []testedRange{t.Claude, t.Codex} {
		if _, ok := parseVersion(r.Min); !ok {
			return t, fmt.Errorf("unreadable tested version %q", r.Min)
		}
		if _, ok := parseVersion(r.Max); !ok {
			return t, fmt.Errorf("unreadable tested version %q", r.Max)
		}
	}
	return t, nil
}

func loadTested() (testedVersions, error) {
	path, err := magiDir("tested.json")
	if err != nil {
		return testedVersions{}, err
	}
	if info, err := os.Stat(path); testedURL != "" && (err != nil || time.Since(info.ModTime()) > testedFresh) {
		if data, err := fetchTested(); err == nil {
			if err := writeAtomic(path, data); err != nil {
				return testedVersions{}, err
			}
		}
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		data = builtinTested
	} else if err != nil {
		return testedVersions{}, err
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

func (r testedRange) refuse(tool, where, version string) error {
	v, ok := parseVersion(version)
	low, _ := parseVersion(r.Min)
	high, _ := parseVersion(r.Max)
	switch {
	case !ok:
		return fmt.Errorf("couldn't read the %s version on %s, so magi can't tell whether it was tested", tool, where)
	case compareVersions(v, low) < 0:
		return fmt.Errorf("%s on %s is %s, older than magi supports; update it to %s or newer", tool, where, version, r.Min)
	case slices.Contains(r.Broken, version):
		return fmt.Errorf("%s %s on %s fails magi's tests, so magi doesn't move sessions with it; update it", tool, version, where)
	case compareVersions(v, high) > 0:
		return fmt.Errorf("%s %s on %s hasn't passed magi's nightly tests yet (tested up to %s); a new version usually passes within a day, so try again later", tool, version, where, r.Max)
	}
	return nil
}

func toolVersion(m Machine, program string) (string, error) {
	out, err := sh(m, `"$1" --version`, program)
	return versionPattern.FindString(string(out)), err
}

func refuseUntested(codex bool, local Machine, remote Machine, remoteProgram string) error {
	t, err := loadTested()
	if err != nil {
		return fmt.Errorf("reading which versions magi was tested with failed: %w", err)
	}
	tool, r, localProgram := "Claude Code", t.Claude, local.Claude()
	if codex {
		tool, r, localProgram = "Codex", t.Codex, local.Codex()
	}
	for _, m := range []struct {
		machine Machine
		program string
	}{{local, localProgram}, {remote, remoteProgram}} {
		version, err := toolVersion(m.machine, m.program)
		if err != nil {
			return fmt.Errorf("reading the %s version on %s failed: %w", tool, m.machine.Name(), err)
		}
		if err := r.refuse(tool, m.machine.Name(), version); err != nil {
			return err
		}
	}
	return nil
}
