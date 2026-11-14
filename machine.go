package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"time"
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

func run(ctx context.Context, name string, args ...string) ([]byte, error) {
	return output(command(ctx, name, args...), name)
}

func command(ctx context.Context, name string, args ...string) *exec.Cmd {
	cmd := exec.CommandContext(ctx, name, args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error { return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL) }
	cmd.WaitDelay = 5 * time.Second
	return cmd
}

func output(cmd *exec.Cmd, what string) ([]byte, error) {
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return out, fmt.Errorf("%s: %w: %s", what, err, strings.TrimSpace(stderr.String()))
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

type Machine interface {
	Name() string
	Context() context.Context
	SetContext(context.Context)
	Command(script string) *exec.Cmd
}

func sh(m Machine, script string) ([]byte, error) {
	return shIn(m, script, nil)
}

func shIn(m Machine, script string, input []byte) ([]byte, error) {
	cmd := m.Command(script)
	if input != nil {
		cmd.Stdin = bytes.NewReader(input)
	}
	out, err := output(cmd, "sh")
	if err != nil {
		return out, fmt.Errorf("on %s: %w", m.Name(), err)
	}
	return out, nil
}

type Local struct {
	ctx  context.Context
	name string
}

func newLocal(ctx context.Context) (*Local, error) {
	name, err := os.Hostname()
	if err != nil {
		return nil, err
	}
	return &Local{ctx: ctx, name: strings.TrimSuffix(name, ".local")}, nil
}

func (l *Local) Name() string                   { return l.name }
func (l *Local) Context() context.Context       { return l.ctx }
func (l *Local) SetContext(ctx context.Context) { l.ctx = ctx }
func (l *Local) Command(script string) *exec.Cmd {
	return command(l.ctx, "sh", "-c", script)
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

func (r *Remote) Name() string                   { return r.host }
func (r *Remote) Context() context.Context       { return r.ctx }
func (r *Remote) SetContext(ctx context.Context) { r.ctx = ctx }
func (r *Remote) Command(script string) *exec.Cmd {
	args := append(append([]string{}, r.opts...), r.host, "--", "LC_ALL=C sh -c "+quote(script))
	return command(r.ctx, "ssh", args...)
}

func (r *Remote) sshCommand() string {
	return "ssh " + quoteAll(r.opts)
}

type Archive struct {
	Paths     []string
	Exclude   []string
	KeepNewer bool
}

func packScript(dir string, a Archive) string {
	var s strings.Builder
	s.WriteString("cd " + quote(dir) + ` && M=; if tar --version 2>/dev/null | grep -q bsdtar; then M=--no-mac-metadata; fi; COPYFILE_DISABLE=1 tar --no-xattrs --no-acls $M`)
	for _, e := range a.Exclude {
		s.WriteString(" --exclude " + quote(e))
	}
	s.WriteString(" --null -T - -cf -")
	return s.String()
}

func unpackScript(dir string, a Archive) string {
	keep := ""
	if a.KeepNewer {
		keep = "--keep-newer-files "
	}
	return "mkdir -p " + quote(dir) + " && tar " + keep + "-xpf - -C " + quote(dir)
}

func transfer(from Machine, fromDir string, a Archive, to Machine, toDir string) error {
	if len(a.Paths) == 0 {
		return nil
	}
	read, write, err := os.Pipe()
	if err != nil {
		return err
	}
	pack := from.Command(packScript(fromDir, a))
	pack.Stdin = strings.NewReader(strings.Join(a.Paths, "\x00") + "\x00")
	pack.Stdout = write
	var packErr bytes.Buffer
	pack.Stderr = &packErr
	unpack := to.Command(unpackScript(toDir, a))
	unpack.Stdin = read
	var unpackErr bytes.Buffer
	unpack.Stderr = &unpackErr

	startErr := pack.Start()
	if startErr == nil {
		startErr = unpack.Start()
	}
	read.Close()
	write.Close()
	if startErr != nil {
		if pack.Process != nil {
			pack.Wait()
		}
		return startErr
	}
	packRunErr := pack.Wait()
	unpackRunErr := unpack.Wait()
	if packRunErr != nil {
		return fmt.Errorf("tar on %s: %w: %s", from.Name(), packRunErr, strings.TrimSpace(packErr.String()))
	}
	if unpackRunErr != nil {
		return fmt.Errorf("unpack on %s: %w: %s", to.Name(), unpackRunErr, strings.TrimSpace(unpackErr.String()))
	}
	return nil
}
