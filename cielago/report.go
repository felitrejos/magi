package main

import (
	"context"
	"fmt"
	"io"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

const (
	tailLines     = 80
	keepCollected = 7 * 24 * time.Hour
)

func tail(text string) string {
	text = strings.ToValidUTF8(text, "�")
	if len(text) >= tailBytes {
		if _, rest, ok := strings.Cut(text, "\n"); ok {
			text = rest
		}
	}
	lines := strings.Split(strings.TrimRight(text, "\n"), "\n")
	if len(lines) > tailLines {
		lines = lines[len(lines)-tailLines:]
	}
	return strings.Join(lines, "\n")
}

func ago(t time.Time) string {
	d := time.Since(t).Round(time.Second)
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%ds ago", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	case d < 48*time.Hour:
		return fmt.Sprintf("%dh%02dm ago", int(d.Hours()), int(d.Minutes())%60)
	}
	return t.Local().Format("2006-01-02 15:04")
}

func fence(text string) string {
	if text == "" {
		return "(no output)\n"
	}
	ticks := "```"
	for strings.Contains(text, ticks) {
		ticks += "`"
	}
	return ticks + "\n" + text + "\n" + ticks + "\n"
}

func describeStarted(s *started, why string) string {
	t := s.Task
	var b strings.Builder
	fmt.Fprintf(&b, "Started task %s (%s) on %s: %s.\n", t.ID, t.Kind, t.Host, why)
	fmt.Fprintf(&b, "It runs in its own copy of the project at %s there, %s, tab %q.\n", t.RunDir, t.Where, t.tabLabel())
	if len(s.ShownOn) > 0 {
		fmt.Fprintf(&b, "It's open in %s.\n", strings.Join(s.ShownOn, ", "))
	}
	for _, n := range s.Notes {
		fmt.Fprintf(&b, "Note: %s.\n", strings.TrimSuffix(n, "."))
	}
	switch t.Kind {
	case kindSession:
		b.WriteString("When the user quits that session, task_result brings its changes back as a branch.\n")
	default:
		fmt.Fprintf(&b, "It doesn't block this session. To be told when it's done, run `cielago tasks wait %s` as a background shell command: it exits when the task finishes. Then collect it with task_result.\n", t.ID)
	}
	return b.String()
}

func (t *Task) tabLabel() string {
	if t.Label != "" {
		return t.Label
	}
	return taskTabLabel(t.Name, t.Kind, t.ID)
}

func describeReport(r *taskReport) string {
	t, st := r.Task, r.State
	var b strings.Builder
	head := fmt.Sprintf("Task %s (%s on %s, started %s): %s", t.ID, t.Kind, t.Host, ago(t.Created), t.summary())
	if t.Turn > 1 {
		head += fmt.Sprintf("\nTurn %d: %s", t.Turn, oneLine(t.Reply))
	}
	switch st.State {
	case "running":
		fmt.Fprintf(&b, "%s\nStill running, %s, tab %q.\n", head, t.Where, t.tabLabel())
		if t.Kind == kindRun || t.Kind == kindDelegate {
			fmt.Fprintf(&b, "Latest output:\n%s", fence(tail(st.Tail)))
		}
		return b.String()
	case "starting":
		fmt.Fprintf(&b, "%s\nStill copying the project to %s.\n", head, t.Host)
		return b.String()
	case "done", "lost", "gone":
		fmt.Fprintf(&b, "%s\n%s. task_result collects it.\n", head, finishedState(st))
		if t.Kind != kindSession {
			fmt.Fprintf(&b, "Last output:\n%s", fence(tail(st.Tail)))
		}
		return b.String()
	}
	o := t.Outcome
	if o == nil {
		return head + "\nState: " + st.State + "\n"
	}
	fmt.Fprintf(&b, "%s\n%s.\n", head, outcomeState(o))
	if t.Kind == kindDelegate && o.Message != "" {
		fmt.Fprintf(&b, "Claude's final message:\n%s\n", strings.TrimSpace(o.Message))
	}
	if t.Kind == kindRun || t.Kind == kindDelegate && o.Message == "" {
		fmt.Fprintf(&b, "Last output:\n%s", fence(tail(o.Tail)))
	}
	if o.Log != "" {
		fmt.Fprintf(&b, "Full log on this machine: %s\n", o.Log)
	}
	switch {
	case o.Branch != "" && o.Diffstat == "" && t.Turn > 1:
		fmt.Fprintf(&b, "This turn changed no files; the task's changes so far are on branch %s.\n", o.Branch)
	case o.Applied:
		fmt.Fprintf(&b, "%s have been applied to your working tree in %s:\n%s", turnChanges(t), t.Project, fence(o.Diffstat))
		fmt.Fprintf(&b, "They're also kept on branch %s. See them: git diff %s %s\n", o.Branch, shortCommit(o.From), o.Branch)
	case o.Unapplied != "":
		fmt.Fprintf(&b, "%s couldn't be applied to your working tree in %s as it is now, so it wasn't touched:\n%s", turnChanges(t), t.Project, fence(o.Unapplied))
		b.WriteString(fence(o.Diffstat))
		fmt.Fprintf(&b, "They're on branch %s. Merge them in yourself, resolving what clashes, then tell the user: git diff --binary %s %s | git apply --3way\n", o.Branch, shortCommit(o.From), o.Branch)
	case o.Branch != "":
		if t.Turn > 1 {
			fmt.Fprintf(&b, "This turn's changes:\n%s", fence(o.Diffstat))
			fmt.Fprintf(&b, "All the task's changes are on branch %s in %s, compared with your working tree when the task started (%s); your working tree wasn't touched.\n", o.Branch, t.Project, shortCommit(t.Base))
		} else {
			fmt.Fprintf(&b, "Its changes are on branch %s in %s, compared with your working tree when the task started (%s); your working tree wasn't touched.\n", o.Branch, t.Project, shortCommit(t.Base))
			if o.Diffstat != "" {
				b.WriteString(fence(o.Diffstat))
			}
		}
		fmt.Fprintf(&b, "See them: git diff %s %s\nApply them here: git diff --binary %s %s | git apply\n", shortCommit(t.Base), o.Branch, shortCommit(t.Base), o.Branch)
	case o.Files != "":
		fmt.Fprintf(&b, "Its copy of the folder, as it left it, is at %s on this machine.\n", o.Files)
	case t.Kind != kindRun && o.State != "cancelled":
		b.WriteString("It changed no files.\n")
	}
	if o.Closed {
		fmt.Fprintf(&b, "Its copy on %s has been removed, so it takes no more follow-ups.\n", t.Host)
	}
	if o.Kept {
		fmt.Fprintf(&b, "Its copy stays on %s so you can send a follow-up with task_reply, which keeps its context. When you're done with it, cancel_task removes the copy; otherwise it's removed once it has been idle for a day.\n", t.Host)
	}
	if t.Subs && t.Kind != kindRun {
		b.WriteString("Changes made inside submodules aren't brought back.\n")
	}
	if o.Note != "" {
		fmt.Fprintf(&b, "Note: %s\n", o.Note)
	}
	return b.String()
}

func turnChanges(t *Task) string {
	if t.Turn > 1 {
		return "This turn's changes"
	}
	return "Its changes"
}

func shortCommit(id string) string {
	return id[:min(12, len(id))]
}

func finishedState(st taskState) string {
	switch st.State {
	case "lost":
		return "Its process is gone without recording an exit status (its tab was closed or the machine restarted)"
	case "gone":
		return "Its copy on that machine is gone"
	}
	return "Finished: " + st.Exit
}

func outcomeState(o *Outcome) string {
	switch o.State {
	case "cancelled":
		return "Cancelled " + ago(o.Ended)
	case "lost":
		return "Collected " + ago(o.Ended) + "; its process had ended without recording an exit status"
	case "gone":
		return "Collected " + ago(o.Ended) + "; its copy was already gone"
	}
	return "Finished (" + o.Exit + "), collected " + ago(o.Ended)
}

func describeMachines(view fleetView, needs needsInput) string {
	var b strings.Builder
	if view.FleetErr != nil {
		fmt.Fprintf(&b, "Live load is unknown: %v. Without magi, cielago picks the first configured machine that answers.\n\n", view.FleetErr)
	}
	if plans := describePlans(view.Plans, time.Now()); plans != "" {
		fmt.Fprintf(&b, "%s\n", plans)
		if v := checkPlan(view.Plans, "claude", time.Now()); v.Refuse != "" {
			b.WriteString("delegate, start_session and task_reply won't start Claude work until it resets, unless the user says to go ahead anyway.\n")
		}
		b.WriteString("\n")
	}
	if view.Here != nil {
		fmt.Fprintf(&b, "This machine (%s): %s\n\n", view.Here.Name, view.Here.describe())
	}
	b.WriteString("Machines cielago can send work to:\n")
	for _, m := range view.Machines {
		fmt.Fprintf(&b, "- %s (ssh %s)", m.Name, m.Host.SSH)
		if m.Reachable != nil {
			fmt.Fprintf(&b, ": NOT reachable over SSH (%v)", m.Reachable)
		} else {
			b.WriteString(": reachable")
		}
		fmt.Fprintf(&b, "; budget: %s", m.Budget)
		if m.NoRoom != "" {
			fmt.Fprintf(&b, "; NO ROOM now: %s", m.NoRoom)
		}
		if m.Fleet != nil {
			fmt.Fprintf(&b, "; %s", strings.Join(strings.Fields(platform(*m.Fleet)+" "+m.Fleet.describe()), " "))
			if len(m.Fleet.Tools) > 0 {
				fmt.Fprintf(&b, "; has %s", strings.Join(m.Fleet.Tools, ", "))
			}
			if load := m.Fleet.sessionLoad(); load != "" {
				fmt.Fprintf(&b, "; %s", load)
			}
		} else if view.FleetErr == nil {
			b.WriteString("; magi doesn't report it, so its load is unknown")
		}
		b.WriteString("\n")
		for _, t := range m.Tasks {
			fmt.Fprintf(&b, "    task %s (%s, started %s): %s\n", t.ID, t.Kind, ago(t.Created), t.summary())
		}
	}
	if !needs.empty() {
		b.WriteString(describePlaces(view, needs))
	} else if picks, _ := view.candidates(); len(picks) > 0 {
		fmt.Fprintf(&b, "\nSuggested: %s, %s.\n", picks[0].Host, picks[0].Why)
	} else {
		b.WriteString("\nNo machine has room right now.\n")
	}
	if len(view.Unmatched) > 0 {
		var names []string
		for _, m := range view.Unmatched {
			names = append(names, m.Name)
		}
		fmt.Fprintf(&b, "\nmagi also sees %s, which isn't a cielago host; add it to the config to send work there.\n", strings.Join(names, ", "))
	}
	return b.String()
}

func printTasks(w io.Writer) error {
	tasks, err := listTasks()
	if err != nil {
		return err
	}
	if len(tasks) == 0 {
		fmt.Fprintln(w, "No tasks yet.")
		return nil
	}
	states := make([]string, len(tasks))
	var wg sync.WaitGroup
	for i, t := range tasks {
		if t.Outcome != nil {
			states[i] = outcomeState(t.Outcome)
			continue
		}
		wg.Go(func() {
			ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
			defer cancel()
			r, _, err := t.remote(ctx)
			if err == nil {
				var st taskState
				if st, err = probeTask(r, t); err == nil {
					err = t.noteState(st.State)
				}
				if err == nil {
					states[i] = st.State
					if st.State == "done" {
						states[i] = "finished (" + st.Exit + "), not collected"
					}
					return
				}
			}
			states[i] = fmt.Sprintf("unknown (%v)", err)
		})
	}
	wg.Wait()
	for i, t := range tasks {
		fmt.Fprintf(w, "%s  %-8s  %-10s  %-16s  %s\n    %s\n", t.ID, t.Kind, t.Host, ago(t.Created), t.summary(), firstLine(states[i]))
	}
	return nil
}

func firstLine(s string) string {
	line, _, _ := strings.Cut(s, "\n")
	if utf8.RuneCountInString(line) > 160 {
		line = string([]rune(line)[:157]) + "..."
	}
	return line
}

func cleanTasks(w io.Writer) error {
	tasks, err := listTasks()
	if err != nil {
		return err
	}
	cleaned := 0
	for _, t := range tasks {
		if t.Outcome != nil {
			continue
		}
		r, err := collectTask(t.ID, false)
		if err != nil {
			fmt.Fprintf(w, "%s: %v\n", t.ID, err)
			continue
		}
		if r.Task.Outcome != nil {
			cleaned++
			fmt.Fprintf(w, "%s: collected (%s)\n", t.ID, outcomeState(r.Task.Outcome))
		}
	}
	pruned, closed := 0, 0
	for _, t := range tasks {
		if t.Outcome != nil && t.Outcome.Kept && time.Since(t.Outcome.Ended) > keepCopies {
			done, err := closeIdleCopy(t.ID, keepCopies)
			if err != nil {
				fmt.Fprintf(w, "%s: %v\n", t.ID, err)
				continue
			}
			if done {
				closed++
			}
			continue
		}
		if t.Outcome != nil && !t.Outcome.Kept && time.Since(t.Outcome.Ended) > keepCollected {
			if err := t.forget(); err != nil {
				fmt.Fprintf(w, "%s: %v\n", t.ID, err)
				continue
			}
			pruned++
		}
	}
	fmt.Fprintf(w, "Collected %d finished task(s); running ones were left alone. Removed %d delegate copies idle for a day, and forgot %d tasks collected more than a week ago.\n", cleaned, closed, pruned)
	return nil
}
