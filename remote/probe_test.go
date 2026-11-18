package main

import (
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

const macSysctl = `#!/bin/sh
for name; do
	case $name in
	hw.ncpu) echo 10 ;;
	vm.loadavg) echo '{ 2.56 2.55 2.64 }' ;;
	hw.memsize) echo 17179869184 ;;
	hw.pagesize) echo 16384 ;;
	vm.page_free_count) echo 12520 ;;
	vm.page_pageable_external_count) echo 180728 ;;
	vm.page_purgeable_count) echo 5018 ;;
	esac
done
`

const macVMStat = `#!/bin/sh
cat <<'EOF'
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                                    13463.
Pages active:                                 244999.
Pages inactive:                               241149.
Pages speculative:                              2852.
Pages throttled:                                   0.
Pages wired down:                             178457.
Pages purgeable:                                5018.
File-backed pages:                            180726.
Anonymous pages:                              308274.
Pages stored in compressor:                   686230.
Pages occupied by compressor:                 327184.
EOF
`

func TestAMacsAvailableMemoryIsItsFreeCachedAndPurgeablePages(t *testing.T) {
	f := newFixture(t)
	f.tool("uname", "#!/bin/sh\ncase $1 in -s) echo Darwin ;; -m) echo arm64 ;; esac\n")
	f.tool("sw_vers", "#!/bin/sh\necho 27.2\n")
	f.tool("sysctl", macSysctl)
	f.tool("vm_stat", macVMStat)
	out, err := sh(f.machine(), platformScript, "claude")
	if err != nil {
		t.Fatal(err)
	}
	m := parsePlatform("mac", string(out))
	if m.OS != "macOS 27.2" || m.Cores != 10 || m.Load[0] != 2.56 {
		t.Fatalf("parsed %+v from %q", m, out)
	}
	if available := m.MemoryTotal - m.MemoryUsed; m.MemoryTotal != 16<<30 || available != (12520+180728+5018)*16384 {
		t.Errorf("total %.0f, available %.0f; want 16 GiB and the free, file-backed and purgeable pages", m.MemoryTotal, available)
	}
}

func TestATargetRechecksMemoryTheWayTheProbeReadsIt(t *testing.T) {
	f := newFixture(t)
	f.tool("uname", "#!/bin/sh\necho Darwin\n")
	f.tool("sysctl", macSysctl)
	f.tool("vm_stat", macVMStat)
	_, err := sh(f.machine(), taskScript, "room", filepath.Join(f.home, ".magi", "tasks", "0000aaaa"), "delegate", "0", strconv.Itoa(4<<30))
	if err == nil || !strings.Contains(err.Error(), "3.0 GB of memory is free there, its budget needs 4.0 GB") {
		t.Errorf("err = %v, want the probe's free memory in the probe's units", err)
	}
}

func TestMemoryIsShownInTheUnitsBudgetsAreWrittenIn(t *testing.T) {
	m := parsePlatform("mac", "macOS 27.2\narm64\n\n10\n2.56 2.55 2.64\n17179869184 3221225472\n0\n")
	got := m.describe()
	if !strings.Contains(got, "memory 13.0 GB of 16.0 GB") {
		t.Fatalf("describe = %q", got)
	}
	if total, err := parseMemory("16.0 GB"); err != nil || total != m.MemoryTotal {
		t.Errorf("the 16.0 GB shown parses back as %v, %v; the machine has %v", total, err, m.MemoryTotal)
	}
	tight := Budget{MinFree: 4 << 30}.noRoom(time.Now(), m, true)
	if tight != "3.0 GB of memory is free there, its budget needs 4.0 GB" {
		t.Errorf("noRoom = %q", tight)
	}
}

func TestDoctorReadsWhetherTheMagiPluginIsInstalledAndEnabled(t *testing.T) {
	f := newFixture(t)
	f.config("local_roots = []\n")
	f.tool("magi-overview", "#!/bin/sh\nexit 1\n")
	f.tool("claude", `#!/bin/sh
case "$1" in
  --version) echo "2.1.325 (Claude Code)" ;;
  auth) echo '{"loggedIn":true,"authMethod":"claude.ai"}' ;;
  plugin) [ "$2 $3" = "list --json" ] && printf '%s' "$MAGI_TEST_PLUGINS" ;;
esac
`)
	for plugins, want := range map[string]string{
		`[{"id":"magi@magi","scope":"user","enabled":true}]`:                                       "✓ the magi plugin is installed and enabled",
		`[{"id":"magi@magi","scope":"user","enabled":false}]`:                                      "✗ the magi plugin is installed but disabled\n      claude plugin enable magi@magi",
		`[{"id":"notmagi@market","scope":"user","enabled":true}]`:                                  "✗ the magi plugin isn't installed",
		`[{"id":"magi@magi","enabled":false},{"id":"magi@magi","scope":"project","enabled":true}]`: "✓ the magi plugin is installed and enabled",
	} {
		t.Setenv("MAGI_TEST_PLUGINS", plugins)
		var out strings.Builder
		doctor(nil, &out)
		if !strings.Contains(out.String(), want) {
			t.Errorf("with %s the report lacks %q:\n%s", plugins, want, out.String())
		}
	}
}

func TestInitSuggestsAFolderReachedTwiceOnlyOnce(t *testing.T) {
	f := newFixture(t)
	if err := os.Symlink("developer", filepath.Join(f.home, "dev")); err != nil {
		t.Fatal(err)
	}
	if got := existing(f.machine(), []string{"~/developer", "~/dev", "~/code", "~/developer"}); !slices.Equal(got, []string{"~/developer"}) {
		t.Errorf("existing = %q", got)
	}
}
