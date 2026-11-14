package consumer

import (
	"bufio"
	"encoding/hex"
	"encoding/json/v2"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

type foundationCase struct {
	Command   []string `json:"command"`
	Rows      int      `json:"rows"`
	Cols      int      `json:"cols"`
	Text      string   `json:"text"`
	OutputHex string   `json:"output_hex"`
	TimeoutMs int      `json:"timeout_ms"`
	Guard     string   `json:"guard"`
	ID        string   `json:"id"`
}

func processIsGone(t *testing.T, pidFile string) bool {
	t.Helper()

	data, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatal(err)
	}

	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		t.Fatal(err)
	}

	_, statErr := os.Stat("/proc/" + strconv.Itoa(pid))

	return pid > 1 && os.IsNotExist(statErr)
}

func TestFoundationCaseMatchesTheSharedFixture(t *testing.T) {
	foundation := fixture[foundationCase](t, "conformance/sdk/cases/foundation.json")
	client, dir := newClient(t)
	pidFile := filepath.Join(dir, "child.pid")
	command := append(slices.Clone(foundation.Command), pidFile)
	recording := launch(
		t,
		client,
		command,
		termtrace.LaunchOptions{Rows: new(foundation.Rows), Cols: new(foundation.Cols)},
	)
	ctx := within(t, 30*time.Second)

	_, err := recording.Screen().
		WaitForText(ctx, foundation.Text, time.Duration(foundation.TimeoutMs)*time.Millisecond)
	fatal(t, err)
	frame, err := recording.Screen().Frame(ctx, termtrace.Projection{})
	fatal(t, err)
	_, err = recording.Actions().IfState(foundation.Guard).SendText("FORBIDDEN\n").Run(ctx)
	stale := errorFacts(t, err)
	page, err := recording.Output().Page(ctx, termtrace.OutputPageOptions{Limit: 4096})
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	if frame.Geometry.Rows != foundation.Rows || frame.Geometry.Cols != foundation.Cols {
		t.Errorf("geometry = %+v", frame.Geometry)
	}

	if !strings.Contains(frame.Text(), foundation.Text) {
		t.Errorf("frame text lacks %q", foundation.Text)
	}

	want := facts{
		Kind:   "stale_state",
		Code:   "stale_observation",
		Effect: map[string]any{"kind": "not_applied"},
		Retry:  "new_request",
	}
	if !sameFacts(stale, want) {
		t.Errorf("stale facts = %+v", stale)
	}

	if got := hex.EncodeToString(page.Bytes); got != foundation.OutputHex {
		t.Errorf("output = %s, want %s", got, foundation.OutputHex)
	}

	if active, err := recording.IsActive(ctx); err != nil || active {
		t.Errorf("active after stop = %v, %v", active, err)
	}

	var completion struct {
		OutputBytes int `json:"output_bytes"`
		Failure     any `json:"failure"`
	}

	data, err := os.ReadFile(filepath.Join(string(recording.Path()), "completion.json"))
	fatal(t, err)
	fatal(t, json.Unmarshal(data, &completion))

	if completion.OutputBytes != len(page.Bytes) || completion.Failure != nil {
		t.Errorf("completion = %+v", completion)
	}

	events, err := os.Open(filepath.Join(string(recording.Path()), "events.jsonl"))
	fatal(t, err)

	defer func() {
		if err := events.Close(); err != nil {
			t.Error(err)
		}
	}()

	scanner := bufio.NewScanner(events)
	scanner.Buffer(nil, 1<<24)
	scanner.Scan()

	for scanner.Scan() {
		var record struct {
			Event struct {
				Type string `json:"type"`
			} `json:"event"`
		}
		if err := json.Unmarshal(scanner.Bytes(), &record); err != nil {
			t.Fatal(err)
		}

		if slices.Contains([]string{"input", "input_intent", "input_result"}, record.Event.Type) {
			t.Errorf("stale guard emitted %s", record.Event.Type)
		}
	}

	if !processIsGone(t, pidFile) {
		t.Error("child was not reaped")
	}

	if !processIsGone(t, pidFile+".recorder") {
		t.Error("recorder was not reaped")
	}
}
