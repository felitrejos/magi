package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
)

func quote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

func quoteAll(args []string) string {
	quoted := make([]string, len(args))
	for i, a := range args {
		quoted[i] = quote(a)
	}
	return strings.Join(quoted, " ")
}

func run(ctx context.Context, dir string, env []string, name string, args ...string) ([]byte, error) {
	cmd := exec.CommandContext(ctx, name, args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), env...)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return out, fmt.Errorf("%s: %w: %s", name, err, strings.TrimSpace(stderr.String()))
	}
	return out, nil
}

func exitCode(err error) int {
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		return exit.ExitCode()
	}
	return -1
}

type Remote struct {
	ctx  context.Context
	host string
	opts []string
}

func newRemote(ctx context.Context, host string) (*Remote, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, err
	}
	sockets := filepath.Join(home, ".cielago", "ssh")
	if err := os.MkdirAll(sockets, 0o700); err != nil {
		return nil, err
	}
	return &Remote{
		ctx:  ctx,
		host: host,
		opts: []string{
			"-o", "BatchMode=yes",
			"-o", "ConnectTimeout=10",
			"-o", "ControlMaster=auto",
			"-o", "ControlPath=" + filepath.Join(sockets, "%C"),
			"-o", "ControlPersist=60",
		},
	}, nil
}

func (r *Remote) sshCommand() string {
	return "ssh " + quoteAll(r.opts)
}

func (r *Remote) args(script string) []string {
	return append(append([]string{}, r.opts...), r.host, "--", "sh -c "+quote(script))
}

func (r *Remote) sh(script string) ([]byte, error) {
	out, err := run(r.ctx, "", nil, "ssh", r.args(script)...)
	if err != nil {
		return out, fmt.Errorf("on %s: %w", r.host, err)
	}
	return out, nil
}

type Archive struct {
	Dir       string
	Paths     []string
	Exclude   []string
	KeepNewer bool
}

func tarCreateArgs(a Archive) []string {
	args := []string{"--no-xattrs", "--no-acls"}
	if runtime.GOOS == "darwin" {
		args = append(args, "--no-mac-metadata")
	}
	for _, e := range a.Exclude {
		args = append(args, "--exclude", e)
	}
	return append(args, "--null", "-T", "-", "-cf", "-")
}

func (r *Remote) copy(a Archive, remoteDir string) error {
	if len(a.Paths) == 0 {
		return nil
	}
	pack := exec.CommandContext(r.ctx, "tar", tarCreateArgs(a)...)
	pack.Dir = a.Dir
	pack.Stdin = strings.NewReader(strings.Join(a.Paths, "\x00") + "\x00")
	pack.Env = append(os.Environ(), "COPYFILE_DISABLE=1")
	var packErr bytes.Buffer
	pack.Stderr = &packErr

	extract := "tar -xpf - -C " + quote(remoteDir)
	if a.KeepNewer {
		extract = "tar --keep-newer-files -xpf - -C " + quote(remoteDir)
	}
	unpack := exec.CommandContext(r.ctx, "ssh", r.args("mkdir -p "+quote(remoteDir)+" && "+extract)...)
	var unpackErr bytes.Buffer
	unpack.Stderr = &unpackErr

	pipe, err := pack.StdoutPipe()
	if err != nil {
		return err
	}
	unpack.Stdin = pipe
	if err := unpack.Start(); err != nil {
		return err
	}
	packRunErr := pack.Run()
	if err := unpack.Wait(); err != nil {
		return fmt.Errorf("unpack on %s: %w: %s", r.host, err, strings.TrimSpace(unpackErr.String()))
	}
	if packRunErr != nil {
		return fmt.Errorf("tar: %w: %s", packRunErr, strings.TrimSpace(packErr.String()))
	}
	return nil
}
