package main

import (
	"fmt"
	"path"
	"sort"
	"strings"
)

const folderRegistry = `"$HOME"/.cielago/folders`

func prune(exclude []string) string {
	if len(exclude) == 0 {
		return ""
	}
	names := make([]string, len(exclude))
	for i, e := range exclude {
		names[i] = "-name " + quote(e)
	}
	return `\( ` + strings.Join(names, " -o ") + ` \) -prune -o `
}

func listFolder(m Machine, dir string, exclude []string) ([]string, error) {
	out, err := sh(m, "cd "+quote(dir)+" && find . "+prune(exclude)+"! -name . -print0")
	if err != nil {
		return nil, err
	}
	var paths []string
	for _, p := range strings.Split(string(out), "\x00") {
		if p != "" {
			paths = append(paths, p)
		}
	}
	return paths, nil
}

func stale(keep, have []string) []string {
	kept := map[string]bool{}
	for _, p := range keep {
		kept[path.Clean(p)] = true
	}
	sort.Strings(have)
	gone := map[string]bool{}
	var remove []string
	for _, p := range have {
		clean := path.Clean(p)
		if kept[clean] || underAny(clean, gone) {
			continue
		}
		gone[clean] = true
		remove = append(remove, p)
	}
	return remove
}

func underAny(p string, dirs map[string]bool) bool {
	for d := path.Dir(p); d != "." && d != "/"; d = path.Dir(d) {
		if dirs[d] {
			return true
		}
	}
	return false
}

func mirrorFolder(from Machine, fromDir string, to Machine, toDir string, exclude []string) error {
	if err := transfer(from, fromDir, Archive{Paths: []string{"."}, Exclude: exclude}, to, toDir); err != nil {
		return err
	}
	keep, err := listFolder(from, fromDir, exclude)
	if err != nil {
		return err
	}
	have, err := listFolder(to, toDir, exclude)
	if err != nil {
		return err
	}
	remove := stale(keep, have)
	if len(remove) == 0 {
		return nil
	}
	_, err = shIn(to, "cd "+quote(toDir)+" && xargs -0 rm -rf --", []byte(strings.Join(remove, "\x00")+"\x00"))
	return err
}

func fingerprint(m Machine, dir string, exclude []string) (string, error) {
	p := prune(exclude)
	out, err := sh(m, "set -e; cd "+quote(dir)+`; t=$(mktemp); trap 'rm -f "$t"' EXIT; `+
		`if stat -c %s . >/dev/null 2>&1; then d='-c %F:%a:%n'; f='-c %F:%a:%s:%Y:%n'; else d='-f %HT:%Lp:%N'; f='-f %HT:%Lp:%z:%m:%N'; fi; `+
		"find . "+p+`-type d ! -name . -exec stat $d {} + > "$t"; find . `+p+`! -type d -exec stat $f {} + >> "$t"; LC_ALL=C sort "$t" | cksum`)
	return strings.TrimSpace(string(out)), err
}

type Folder struct {
	State      string
	Recorded   string
	Origin     string
	RootCommit string
}

const legacyEntry = "-"

func folderState(m Machine, dir string) (Folder, error) {
	out, err := sh(m, "p="+quote(dir)+`; if [ ! -e "$p" ]; then echo new; exit 0; fi; p=$(cd "$p" && pwd -P); `+
		`if [ -e "$p/.git" ]; then printf 'git\t%s\t%s\n' "$(git -C "$p" remote get-url origin 2>/dev/null || :)" "$(git -C "$p" rev-list --max-parents=0 HEAD 2>/dev/null | LC_ALL=C sort | head -n 1)"; exit 0; fi; `+
		`r=; if [ -f `+folderRegistry+` ]; then r=$(p="$p" awk -F '\t' '$NF == ENVIRON["p"] {f = (NF == 1 ? "`+legacyEntry+`" : $1)} END {print f}' `+folderRegistry+`); fi; `+
		`if [ -n "$r" ]; then printf 'ours\t%s\n' "$r"; else echo foreign; fi`)
	if err != nil {
		return Folder{}, err
	}
	fields := strings.Split(strings.TrimRight(string(out), "\n"), "\t")
	f := Folder{State: fields[0]}
	switch {
	case f.State == "ours" && len(fields) == 2:
		f.Recorded = fields[1]
	case f.State == "git" && len(fields) == 3:
		f.Origin, f.RootCommit = fields[1], fields[2]
	}
	return f, nil
}

func registerFolder(m Machine, dir string, exclude []string) error {
	print, err := fingerprint(m, dir, exclude)
	if err != nil {
		return err
	}
	_, err = sh(m, `set -e; p=$(cd `+quote(dir)+` && pwd -P); mkdir -p "$HOME"/.cielago; touch `+folderRegistry+`; t=$(mktemp); `+
		`p="$p" awk -F '\t' '$NF != ENVIRON["p"]' `+folderRegistry+` > "$t"; printf '%s\t%s\n' `+quote(print)+` "$p" >> "$t"; mv "$t" `+folderRegistry)
	return err
}

func folderUnchanged(m Machine, dir, recorded string, exclude []string) error {
	if recorded == legacyEntry {
		return fmt.Errorf("%s on %s was left by an older cielago that didn't record what was in it; check it, then move it away or delete its line in ~/.cielago/folders", dir, m.Name())
	}
	now, err := fingerprint(m, dir, exclude)
	if err != nil {
		return err
	}
	if now != recorded {
		return fmt.Errorf("%s on %s changed since a session last left it there; move your changes aside or hand off from a folder with another name", dir, m.Name())
	}
	return nil
}
