package sh

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

	"github.com/felitrejos/magi/remote/internal/store"
)

func Quote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

func quoteAll(args []string) string {
	quoted := make([]string, len(args))
	for i, a := range args {
		quoted[i] = Quote(a)
	}
	return strings.Join(quoted, " ")
}

func Exec(ctx context.Context, name string, args ...string) ([]byte, error) {
	return output(command(ctx, name, args...), name)
}

func command(ctx context.Context, name string, args ...string) *exec.Cmd {
	cmd := exec.CommandContext(ctx, name, args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error { return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL) }
	cmd.WaitDelay = 5 * time.Second
	return cmd
}

type Error struct {
	What   string
	Stderr string
	Err    error
}

func (e *Error) Error() string {
	if e.Stderr == "" {
		return e.What + ": " + e.Err.Error()
	}
	var exit *exec.ExitError
	if errors.As(e.Err, &exit) && exit.ExitCode() >= 0 {
		return e.Stderr
	}
	return e.Stderr + " (" + e.Err.Error() + ")"
}

func (e *Error) Unwrap() error { return e.Err }

func output(cmd *exec.Cmd, what string) ([]byte, error) {
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return out, &Error{What: what, Stderr: strings.TrimSpace(stderr.String()), Err: err}
	}
	return out, nil
}

const (
	ExitExists      = 7
	ExitConflicts   = 8
	ExitInProgress  = 9
	ExitBusy        = 10
	ExitGone        = 11
	ExitNoRoom      = 11
	ExitUnreachable = 255
)

func ExitCode(err error) int {
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		return exit.ExitCode()
	}
	return -1
}

type Machine interface {
	Name() string
	Context() context.Context
	With(context.Context) Machine
	Command(script string, args ...string) *exec.Cmd
	Claude() string
	Codex() string
}

func Run(m Machine, script string, args ...string) ([]byte, error) {
	return RunInput(m, script, nil, args...)
}

func RunInput(m Machine, script string, input []byte, args ...string) ([]byte, error) {
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
	ctx   context.Context
	name  string
	codex string
}

func NewLocal(ctx context.Context) (*Local, error) {
	name, err := os.Hostname()
	if err != nil {
		return nil, err
	}
	codex, _ := exec.LookPath("codex")
	return &Local{ctx: ctx, name: strings.TrimSuffix(name, ".local"), codex: codex}, nil
}

func (l *Local) Name() string             { return l.name }
func (l *Local) Context() context.Context { return l.ctx }
func (l *Local) Claude() string           { return "claude" }
func (l *Local) Codex() string            { return l.codex }
func (l *Local) With(ctx context.Context) Machine {
	c := *l
	c.ctx = ctx
	return &c
}
func (l *Local) Command(script string, args ...string) *exec.Cmd {
	return command(l.ctx, "sh", append([]string{"-c", script, "magi"}, args...)...)
}

type Remote struct {
	ctx    context.Context
	host   string
	opts   []string
	claude string
	codex  string
}

func NewRemote(ctx context.Context, ssh, claude, codex string) (*Remote, error) {
	sockets, err := store.Dir("ssh")
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(sockets, 0o700); err != nil {
		return nil, err
	}
	return &Remote{
		ctx:    ctx,
		host:   ssh,
		claude: claude,
		codex:  codex,
		opts: []string{
			"-o", "BatchMode=yes",
			"-o", "ConnectTimeout=10",
			"-o", "ControlMaster=auto",
			"-o", "ControlPath=" + filepath.Join(sockets, "%C"),
			"-o", "ControlPersist=60",
			"-o", "ServerAliveInterval=15",
			"-o", "ServerAliveCountMax=3",
		},
	}, nil
}

func (r *Remote) Name() string             { return r.host }
func (r *Remote) Context() context.Context { return r.ctx }
func (r *Remote) Claude() string           { return r.claude }
func (r *Remote) Codex() string            { return r.codex }
func (r *Remote) With(ctx context.Context) Machine {
	c := *r
	c.ctx = ctx
	return &c
}
func (r *Remote) Command(script string, args ...string) *exec.Cmd {
	payload := "set -- " + quoteAll(args) + "\n" + script
	line := `sh -c 'eval "$(LC_ALL=C printf "$1")"' magi '` + printfEscape(payload) + `'`
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

func (r *Remote) SSHCommand() string {
	return "ssh " + quoteAll(r.opts)
}

type Archive struct {
	Paths     []string
	Exclude   []string
	KeepNewer bool
	Follow    bool
}

func Transfer(from Machine, fromDir string, a Archive, to Machine, toDir string) error {
	if len(a.Paths) == 0 {
		return nil
	}
	read, write, err := os.Pipe()
	if err != nil {
		return err
	}
	follow := "0"
	if a.Follow {
		follow = "1"
	}
	pack := from.Command(transferScript, append([]string{"pack", fromDir, follow}, a.Exclude...)...)
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
		errs = append(errs, fmt.Errorf("unpacking on %s: %w", to.Name(), &Error{What: "tar", Stderr: strings.TrimSpace(unpackErr.String()), Err: err}))
	}
	if err := pack.Wait(); err != nil {
		errs = append(errs, fmt.Errorf("packing on %s: %w", from.Name(), &Error{What: "tar", Stderr: strings.TrimSpace(packErr.String()), Err: err}))
	}
	return errors.Join(errs...)
}
