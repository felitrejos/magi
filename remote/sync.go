package main

import (
	"errors"
	"fmt"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func sourceWorkspace(m sh.Machine, machine, cwd string, secrets, exclude []string) (Workspace, string, error) {
	ws, err := readWorkspace(m, cwd, secrets, exclude)
	if err != nil {
		return ws, "", err
	}
	home, err := homeDir(m)
	if err != nil {
		return ws, "", err
	}
	if ws.Git == nil && (ws.Root == home || ws.Root == "/") {
		return ws, "", fmt.Errorf("%s on %s is a whole home folder, not a project; start Claude in the project's folder to hand it off", ws.Root, machine)
	}
	return ws, home, nil
}

func syncProject(from, to sh.Machine, ws Workspace, where Placement, exclude []string, syncRepo func() error) error {
	if ws.Git == nil {
		if where.NewFolder != "" {
			if err := claimNewFolder(to, where.NewFolder); err != nil {
				return err
			}
		}
		return errors.Join(mirrorFolder(from, ws.Root, to, where.Root, exclude), registerFolder(to, where.Root, exclude))
	}
	if where.NewFolder != "" {
		if err := createClone(to, where.NewFolder, ws.Git.Origin); err != nil {
			return err
		}
	} else if err := checkoutAccepts(to, where.Root, ws); err != nil {
		return err
	}
	return syncRepo()
}

func ourFolderUnchanged(m sh.Machine, dir string, exclude []string) error {
	f, err := folderState(m, dir)
	if err != nil {
		return err
	}
	return folderUnchanged(m, dir, f.Recorded, exclude)
}

func settle(source sh.Machine, ws Workspace, exclude []string) error {
	if ws.Git != nil {
		return park(source, ws.Root, ws)
	}
	return registerFolder(source, ws.Root, exclude)
}

func moveTranscript(from sh.Machine, transcriptPath string, ws Workspace, to sh.Machine, where Placement, toDir string) (sidecar string, err error) {
	toProject, err := claudeProject(where.Home, toDir)
	if err != nil {
		return "", err
	}
	set, err := sessionFiles(from, transcriptPath)
	if err != nil {
		return "", err
	}
	if set.Subagents {
		sidecar = toProject + "/" + set.Files[1]
	}
	if err := sh.Transfer(from, set.Dir, sh.Archive{Paths: set.Files}, to, toProject); err != nil {
		return "", err
	}
	fromHome, err := homeDir(from)
	if err != nil {
		return "", err
	}
	fromMemory, err := claudeProject(fromHome, ws.MainRoot)
	if err != nil {
		return "", err
	}
	toMemory, err := claudeProject(where.Home, where.Root)
	if err != nil {
		return "", err
	}
	return sidecar, syncMemory(from, fromMemory+"/memory", to, toMemory+"/memory")
}
