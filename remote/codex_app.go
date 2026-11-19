package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"

	"github.com/felitrejos/magi/remote/internal/sh"
)

const codexHello = `{"id":0,"method":"initialize","params":{"clientInfo":{"name":"magi","version":"1"}}}
{"method":"initialized"}
`

type codexCall struct {
	ID     int    `json:"id"`
	Method string `json:"method"`
	Params any    `json:"params"`
}

type codexThread struct {
	ID   string `json:"id"`
	Cwd  string `json:"cwd"`
	Name string `json:"name"`
	Path string `json:"path"`
	Busy bool
}

func askCodex(m sh.Machine, calls ...codexCall) (string, []json.RawMessage, error) {
	cmd := m.Command(codexScript, "app", m.Codex())
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return "", nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return "", nil, err
	}
	if err := cmd.Start(); err != nil {
		return "", nil, err
	}
	home, answers, err := converse(stdin, stdout, calls)
	stdin.Close()
	io.Copy(io.Discard, stdout)
	if waitErr := cmd.Wait(); err == nil {
		err = waitErr
	}
	if err != nil {
		return "", nil, fmt.Errorf("asking Codex on %s: %w", m.Name(), &sh.Error{What: "codex app-server", Stderr: strings.TrimSpace(stderr.String()), Err: err})
	}
	return home, answers, nil
}

func converse(stdin io.Writer, stdout io.Reader, calls []codexCall) (string, []json.RawMessage, error) {
	request := []byte(codexHello)
	for i := range calls {
		calls[i].ID = i + 1
		line, err := json.Marshal(calls[i])
		if err != nil {
			return "", nil, err
		}
		request = append(append(request, line...), '\n')
	}
	if _, err := stdin.Write(request); err != nil {
		return "", nil, err
	}
	var hello struct {
		CodexHome string `json:"codexHome"`
	}
	answers := make([]json.RawMessage, len(calls))
	dec := json.NewDecoder(stdout)
	for left := len(calls) + 1; left > 0; {
		var reply struct {
			ID     int             `json:"id"`
			Method string          `json:"method"`
			Result json.RawMessage `json:"result"`
			Error  *struct {
				Message string `json:"message"`
			} `json:"error"`
		}
		if err := dec.Decode(&reply); err != nil {
			if errors.Is(err, io.EOF) {
				return "", nil, errors.New("it stopped before it answered")
			}
			return "", nil, err
		}
		if reply.Method != "" || reply.ID < 0 || reply.ID > len(calls) {
			continue
		}
		if reply.Error != nil {
			method := "initialize"
			if reply.ID > 0 {
				method = calls[reply.ID-1].Method
			}
			return "", nil, &codexRefusal{Method: method, Message: reply.Error.Message}
		}
		if reply.ID == 0 {
			if err := json.Unmarshal(reply.Result, &hello); err != nil {
				return "", nil, err
			}
		} else {
			answers[reply.ID-1] = reply.Result
		}
		left--
	}
	return hello.CodexHome, answers, nil
}

type codexRefusal struct {
	Method, Message string
}

func (e *codexRefusal) Error() string {
	return e.Method + ": " + e.Message
}

func openCodexSessions(m sh.Machine) ([]codexThread, error) {
	out, err := sh.Run(m, codexScript, "opened")
	if err != nil {
		return nil, err
	}
	ids := strings.Fields(string(out))
	if len(ids) == 0 {
		return nil, nil
	}
	calls := make([]codexCall, len(ids))
	for i, id := range ids {
		calls[i] = codexCall{Method: "thread/read", Params: map[string]string{"threadId": id}}
	}
	_, answers, err := askCodex(m, calls...)
	if err != nil {
		return nil, err
	}
	threads := make([]codexThread, len(answers))
	for i, answer := range answers {
		var read struct {
			Thread codexThread `json:"thread"`
		}
		if err := json.Unmarshal(answer, &read); err != nil {
			return nil, fmt.Errorf("unreadable Codex session %s on %s: %w", ids[i], m.Name(), err)
		}
		turn, err := openTurn(m, read.Thread.Path)
		if err != nil {
			return nil, err
		}
		read.Thread.Busy = turn != ""
		threads[i] = read.Thread
	}
	return threads, nil
}
