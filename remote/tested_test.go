package main

import (
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/store"
)

func TestOnlyTestedVersionsAreAccepted(t *testing.T) {
	r := testedRange{Min: "2.1.300", Max: "2.1.327", Broken: []string{"2.1.310"}}
	for version, want := range map[string]string{
		"2.1.300": "",
		"2.1.327": "",
		"2.1.299": "older than magi supports",
		"2.1.310": "Claude Code 2.1.310 on homelab fails magi's tests",
		"2.1.328": "Claude Code 2.1.328 on homelab hasn't passed magi's nightly tests yet (tested up to 2.1.327)",
		"":        "couldn't read",
	} {
		err := r.refuse("Claude Code", "homelab", version)
		if want == "" && err != nil || want != "" && (err == nil || !strings.Contains(err.Error(), want)) {
			t.Errorf("version %q: err = %v, want %q", version, err, want)
		}
	}
}

func TestTheTestedListIsFetchedAtMostHourlyAndTheBuiltInOneServesOffline(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	var calls atomic.Int32
	live := `{"claude":{"min":"2.1.300","max":"2.1.400"},"codex":{"min":"0.1.0","max":"0.200.0"}}`
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		calls.Add(1)
		w.Write([]byte(live))
	}))
	defer server.Close()
	url := testedURL
	t.Cleanup(func() { testedURL = url })

	testedURL = server.URL + "/missing"
	server.Config.Handler = http.NotFoundHandler()
	got, err := loadTested()
	builtin, _ := parseTested(builtinTested)
	if err != nil || got.Claude.Max != builtin.Claude.Max {
		t.Fatalf("offline: %+v, %v; want the built-in list", got, err)
	}

	server.Config.Handler = http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		calls.Add(1)
		w.Write([]byte(live))
	})
	testedURL = server.URL
	for range 2 {
		if got, err = loadTested(); err != nil || got.Claude.Max != "2.1.400" {
			t.Fatalf("online: %+v, %v", got, err)
		}
	}
	if calls.Load() != 1 {
		t.Errorf("fetched %d times within the hour, want once", calls.Load())
	}
	path, _ := store.Dir("tested.json")
	old := time.Now().Add(-2 * testedFresh)
	if err := os.Chtimes(path, old, old); err != nil {
		t.Fatal(err)
	}
	server.Config.Handler = http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.Write([]byte("<html>")) })
	if got, err = loadTested(); err != nil || got.Claude.Max != "2.1.400" {
		t.Errorf("a garbled answer replaced the cached list: %+v, %v", got, err)
	}
}

func TestAHandoffRefusesAClaudeCodeThatHasntBeenTested(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-eeeeeeeeeeee"
	h := f.handoffToServer(id)
	t.Setenv("MAGI_FAKE_CLAUDE_VERSION", "9.9.9")
	res, err := h.Run(t.Context())
	if err == nil || !strings.Contains(err.Error(), "hasn't passed magi's nightly tests") {
		t.Fatalf("err = %v, want the untested version refused", err)
	}
	if len(res.Steps) != 1 || res.Steps[0].Name != "inspect" {
		t.Errorf("steps = %+v, want it to stop at inspect", res.Steps)
	}
}

func TestAPullRefusesACodexThatHasntBeenTested(t *testing.T) {
	f := newFixture(t)
	t.Setenv("MAGI_TEST_REMOTE_CODEX_HOME", f.remoteCodexHome())
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71e3"
	f.codexRollout(f.remoteCodexHome(), id, f.remote, longAgo, false)
	t.Setenv("MAGI_FAKE_CODEX_VERSION", "9.9.9")
	res, err := Pull{HostName: "server", Host: Host{SSH: "fakehost", Claude: "claude", Codex: "codex", Roots: []string{"~/developer"}, LocalRoots: []string{"~/laptop"}}, SessionID: id, Out: io.Discard}.Run()
	if err == nil || !strings.Contains(err.Error(), "Codex 9.9.9") {
		t.Fatalf("err = %v, want the untested Codex refused", err)
	}
	if len(res.Steps) != 1 || res.Steps[0].Name != "find" {
		t.Errorf("steps = %+v, want it to stop at find", res.Steps)
	}
}
