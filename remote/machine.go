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

type commandError struct {
	what   string
	stderr string
	err    error
}

func (e *commandError) Error() string {
	if e.stderr == "" {
		return e.what + ": " + e.err.Error()
	}
	var exit *exec.ExitError
	if errors.As(e.err, &exit) && exit.ExitCode() >= 0 {
		return e.stderr
	}
	return e.stderr + " (" + e.err.Error() + ")"
}

func (e *commandError) Unwrap() error { return e.err }

func output(cmd *exec.Cmd, what string) ([]byte, error) {
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return out, &commandError{what: what, stderr: strings.TrimSpace(stderr.String()), err: err}
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
	Command(script string, args ...string) *exec.Cmd
}

func sh(m Machine, script string, args ...string) ([]byte, error) {
	return shIn(m, script, nil, args...)
}

func shIn(m Machine, script string, input []byte, args ...string) ([]byte, error) {
	cmd := m.Command(script, args...)
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
func (l *Local) Command(script string, args ...string) *exec.Cmd {
	return command(l.ctx, "sh", append([]string{"-c", script, "magi"}, args...)...)
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
	sockets := filepath.Join(home, ".magi", "ssh")
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
func (r *Remote) Command(script string, args ...string) *exec.Cmd {
	payload := "set -- " + quoteAll(args) + "\n" + script
	line := `env LC_ALL=C sh -c 'eval "$(printf "$1")"' magi '` + printfEscape(payload) + `'`
	return command(r.ctx, "ssh", append(append([]string{}, r.opts...), r.host, "--", line)...)
}

func printfEscape(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c < 0x20 || c >= 0x7f || strings.IndexByte(`'\%!`, c) >= 0 {
			fmt.Fprintf(&b, `\%03o`, c)
			continue
		}
		b.WriteByte(c)
	}
	return b.String()
}

func (r *Remote) sshCommand() string {
	return "ssh " + quoteAll(r.opts)
}

type Archive struct {
	Paths     []string
	Exclude   []string
	KeepNewer bool
}

func transfer(from Machine, fromDir string, a Archive, to Machine, toDir string) error {
	if len(a.Paths) == 0 {
		return nil
	}
	read, write, err := os.Pipe()
	if err != nil {
		return err
	}
	pack := from.Command(transferScript, append([]string{"pack", fromDir}, a.Exclude...)...)
	pack.Stdin = strings.NewReader(strings.Join(a.Paths, "\x00") + "\x00")
	pack.Stdout = write
	var packErr bytes.Buffer
	pack.Stderr = &packErr
	keep := "0"
	if a.KeepNewer {
		keep = "1"
	}
	unpack := to.Command(transferScript, "unpack", toDir, keep)
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
	var errs []error
	if err := unpack.Wait(); err != nil {
		errs = append(errs, fmt.Errorf("unpacking on %s: %w", to.Name(), &commandError{what: "tar", stderr: strings.TrimSpace(unpackErr.String()), err: err}))
	}
	if err := pack.Wait(); err != nil {
		errs = append(errs, fmt.Errorf("packing on %s: %w", from.Name(), &commandError{what: "tar", stderr: strings.TrimSpace(packErr.String()), err: err}))
	}
	return errors.Join(errs...)
}
