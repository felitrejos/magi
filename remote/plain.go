package main

import (
	"fmt"
	"path"
	"sort"
	"strings"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func pruneArgs(exclude []string) []string {
	if len(exclude) == 0 {
		return nil
	}
	args := []string{"("}
	for i, e := range exclude {
		if i > 0 {
			args = append(args, "-o")
		}
		args = append(args, "-name", e)
	}
	return append(args, ")", "-prune", "-o")
}

func folder(m sh.Machine, command, dir string, args ...string) ([]byte, error) {
	return sh.Run(m, folderScript, append([]string{command, dir}, args...)...)
}

func listFolder(m sh.Machine, dir string, exclude []string) ([]string, error) {
	out, err := folder(m, "list", dir, pruneArgs(exclude)...)
	if err != nil {
		return nil, err
	}
	return splitNull(out), nil
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

func mirrorFolder(from sh.Machine, fromDir string, to sh.Machine, toDir string, exclude []string) error {
	if err := sh.Transfer(from, fromDir, sh.Archive{Paths: []string{"."}, Exclude: exclude}, to, toDir); err != nil {
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
	_, err = sh.RunInput(to, folderScript, []byte(strings.Join(remove, "\x00")+"\x00"), "remove", toDir)
	return err
}

func fingerprint(m sh.Machine, dir string, exclude []string) (string, error) {
	out, err := folder(m, "fingerprint", dir, pruneArgs(exclude)...)
	return strings.TrimSpace(string(out)), err
}

type Folder struct {
	State      string
	Recorded   string
	Origin     string
	RootCommit string
}

func folderState(m sh.Machine, dir string) (Folder, error) {
	out, err := folder(m, "state", dir)
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

func registerFolder(m sh.Machine, dir string, exclude []string) error {
	recorded, err := fingerprint(m, dir, exclude)
	if err != nil {
		return err
	}
	_, err = folder(m, "register", dir, recorded)
	return err
}

func folderUnchanged(m sh.Machine, dir, recorded string, exclude []string) error {
	now, err := fingerprint(m, dir, exclude)
	if err != nil {
		return err
	}
	if now != recorded {
		return fmt.Errorf("%s on %s changed since a session last left it there; move your changes aside or hand off from a folder with another name", dir, m.Name())
	}
	return nil
}
