package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

var tryProject = regexp.MustCompile(`^magi-try-[0-9a-f]{8}$`)

var tryWords = []string{"HERON", "OTTER", "BADGER", "KESTREL", "MARMOT", "IBIS"}

const trySetupScript = `set -e
mkdir -p "$1"
cd "$1"
git init -q
printf 'base\n' > notes.txt
git add notes.txt
git -c user.name="magi try" -c user.email=try@magi.invalid commit -q -m base
printf 'uncommitted line\n' >> notes.txt`

const tryCleanupScript = `project=$1 id=$2
shift 2
for root; do
  case $root in "~/"*) root=$HOME/${root#"~/"} ;; esac
  rm -rf -- "$root/$project"
done
rm -rf -- "$HOME/.claude/projects/"*"$project"
[ -z "$id" ] || rm -f -- "$HOME/.magi/moved/$id"`

type claudeReply struct {
	SessionID string `json:"session_id"`
	Result    string `json:"result"`
	IsError   bool   `json:"is_error"`
}

func claudeEnv() []string {
	var env []string
	for _, kv := range os.Environ() {
		if strings.HasPrefix(kv, "CLAUDE_CODE_") || strings.HasPrefix(kv, "CLAUDECODE=") {
			continue
		}
		env = append(env, kv)
	}
	return env
}

func tryCommand(args []string, in io.Reader, out io.Writer) error {
	if len(args) > 1 {
		return errors.New("usage: magi try [host]")
	}
	cfg, _, err := loadConfig()
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	checked := len(cfg.Order) == 0
	if checked {
		if err := initConfig(in, out); err != nil {
			return err
		}
		if cfg, _, err = loadConfig(); err != nil {
			return err
		}
	}
	name := cfg.Order[0]
	if len(args) == 1 {
		name = args[0]
	}
	if !checked {
		if err := doctor([]string{name}, out); err != nil {
			return fmt.Errorf("%w; fix what failed above, then run magi try again", err)
		}
	}
	return tryRoundTrip(cfg, name, out)
}

func tryRoundTrip(cfg Config, name string, out io.Writer) (err error) {
	host, err := loadHost(name)
	if err != nil {
		return err
	}
	local, err := newLocal(context.Background())
	if err != nil {
		return err
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	project := "magi-try-" + newTaskID()
	dir := filepath.Join(expandHome(cfg.LocalRoots[0], home), project)
	word := tryWords[rand.IntN(len(tryWords))]
	fmt.Fprintf(out, "\nTrying a round trip to %s with a throwaway session in %s. It takes three short Claude turns on your account.\n\n", name, dir)
	var id string
	defer func() { err = errors.Join(err, tryCleanup(local, cfg.LocalRoots, host, project, id)) }()
	if _, err := sh(local, trySetupScript, dir); err != nil {
		return fmt.Errorf("creating the test project failed: %w", err)
	}
	started, err := tryClaude(dir, "Remember the word "+word+". Reply with just: ok")
	if err != nil {
		return fmt.Errorf("starting a Claude session here failed: %w", err)
	}
	id = started.SessionID
	if err := (mover{session: id, host: name, out: out}).run(); err != nil {
		return fmt.Errorf("moving the session to %s failed, as said above", name)
	}
	if _, err := pullHere(name, id, "", out, false); err != nil {
		return fmt.Errorf("bringing the session back from %s failed: %w", name, err)
	}
	checked, err := tryClaude(dir, "--resume", id, "What word did I ask you to remember? Reply with just the word.")
	if err != nil {
		return fmt.Errorf("resuming the session here failed: %w", err)
	}
	if !strings.Contains(strings.ToUpper(checked.Result), word) {
		return fmt.Errorf("the session came back but didn't remember %s; it said %q", word, checked.Result)
	}
	if notes, err := os.ReadFile(filepath.Join(dir, "notes.txt")); err != nil || string(notes) != "base\nuncommitted line\n" {
		return fmt.Errorf("the session came back, but its uncommitted change didn't: notes.txt is %q (%v)", notes, err)
	}
	fmt.Fprintf(out, "\nmagi works with %s: the session went there and back with its uncommitted change, and still remembered %s.\n", name, word)
	return nil
}

func tryClaude(dir string, args ...string) (claudeReply, error) {
	cmd := exec.Command("claude", append([]string{"-p", "--model", "haiku", "--output-format", "json"}, args...)...)
	cmd.Dir, cmd.Env = dir, claudeEnv()
	data, err := cmd.Output()
	var reply claudeReply
	if err == nil {
		err = json.Unmarshal(data, &reply)
	}
	if err == nil && (reply.IsError || reply.SessionID == "") {
		err = fmt.Errorf("claude answered %q", reply.Result)
	}
	return reply, err
}

func tryCleanup(local *Local, localRoots []string, host Host, project, sessionID string) error {
	if !tryProject.MatchString(project) {
		return fmt.Errorf("refusing to remove %q, which isn't a magi try project", project)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	var errs []error
	if remote, err := newRemote(ctx, host); err != nil {
		errs = append(errs, err)
	} else {
		if sessionID != "" {
			errs = append(errs, closeRecordedTab(remote, sessionID))
		}
		_, err := sh(remote, tryCleanupScript, append([]string{project, sessionID}, host.Roots...)...)
		errs = append(errs, err)
	}
	_, err := sh(local, tryCleanupScript, append([]string{project, sessionID}, localRoots...)...)
	if err := errors.Join(append(errs, err)...); err != nil {
		return fmt.Errorf("removing the test project %s failed: %w", project, err)
	}
	return nil
}
