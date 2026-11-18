package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
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
}

func askCodex(m Machine, program string, calls ...codexCall) ([]json.RawMessage, error) {
	cmd := m.Command(codexScript, "app", program)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	answers, err := converse(stdin, stdout, calls)
	stdin.Close()
	io.Copy(io.Discard, stdout)
	if waitErr := cmd.Wait(); err == nil {
		err = waitErr
	}
	if err != nil {
		return nil, fmt.Errorf("asking Codex on %s: %w", m.Name(), &commandError{what: "codex app-server", stderr: strings.TrimSpace(stderr.String()), err: err})
	}
	return answers, nil
}

func converse(stdin io.Writer, stdout io.Reader, calls []codexCall) ([]json.RawMessage, error) {
	request := []byte(codexHello)
	for i := range calls {
		calls[i].ID = i + 1
		line, err := json.Marshal(calls[i])
		if err != nil {
			return nil, err
		}
		request = append(append(request, line...), '\n')
	}
	if _, err := stdin.Write(request); err != nil {
		return nil, err
	}
	answers := make([]json.RawMessage, len(calls))
	dec := json.NewDecoder(stdout)
	for left := len(calls); left > 0; {
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
				return nil, errors.New("it stopped before it answered")
			}
			return nil, err
		}
		if reply.Method != "" || reply.ID < 1 || reply.ID > len(calls) {
			continue
		}
		if reply.Error != nil {
			return nil, fmt.Errorf("%s: %s", calls[reply.ID-1].Method, reply.Error.Message)
		}
		answers[reply.ID-1] = reply.Result
		left--
	}
	return answers, nil
}

func openCodexSessions(m Machine, program string) ([]codexThread, error) {
	out, err := sh(m, codexScript, "opened")
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
	answers, err := askCodex(m, program, calls...)
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
		threads[i] = read.Thread
	}
	return threads, nil
}
