package main

import (
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
		"2.1.310": "homelab has Claude Code 2.1.310, which fails magi's tests",
		"2.1.328": "homelab has Claude Code 2.1.328, which hasn't passed magi's nightly tests yet (tested up to 2.1.327)",
		"":        "couldn't read",
	} {
		err := r.refuse("homelab", version)
		if want == "" && err != nil || want != "" && (err == nil || !strings.Contains(err.Error(), want)) {
			t.Errorf("version %q: err = %v, want %q", version, err, want)
		}
	}
}

func TestTheTestedListIsFetchedAtMostHourlyAndTheBuiltInOneServesOffline(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	var calls atomic.Int32
	live := `{"claude":{"min":"2.1.300","max":"2.1.400"}}`
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
	if err != nil || got.Max != builtin.Max {
		t.Fatalf("offline: %+v, %v; want the built-in list", got, err)
	}

	server.Config.Handler = http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		calls.Add(1)
		w.Write([]byte(live))
	})
	testedURL = server.URL
	for range 2 {
		if got, err = loadTested(); err != nil || got.Max != "2.1.400" {
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
	if got, err = loadTested(); err != nil || got.Max != "2.1.400" {
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
