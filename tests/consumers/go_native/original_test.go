package consumer

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

const winchLoop = "trap 'echo winch $(stty size)' WINCH; echo ready; while :; do sleep 0.05; done"

func originalOptions() termtrace.OriginalLaunchOptions {
	return termtrace.OriginalLaunchOptions{
		Grid:             &termtrace.CellGrid{Cols: 80, Rows: 24},
		Canvas:           &termtrace.PixelCanvas{Width: 1280, Height: 720},
		StartupTimeoutMs: new(uint64(60000)),
	}
}

// owned force-closes original at cleanup when this client still owns it.
func owned(t *testing.T, original *termtrace.OriginalSession) *termtrace.OriginalSession {
	t.Helper()

	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		if active, err := original.IsActive(ctx); err == nil && active {
			_ = original.ForceClose(ctx)
		}
	})

	return original
}

func recordOriginal(
	t *testing.T,
	client *termtrace.Client,
	command []string,
	options termtrace.OriginalLaunchOptions,
) *termtrace.OriginalSession {
	t.Helper()

	original, err := client.RecordOriginal(within(t, 90*time.Second), command, options)
	fatal(t, err)

	return owned(t, original)
}

func attachOriginal(
	t *testing.T,
	client *termtrace.Client,
	path string,
) *termtrace.OriginalSession {
	t.Helper()

	original, err := client.AttachOriginal(within(t, 30*time.Second), path)
	fatal(t, err)

	return owned(t, original)
}

func waitForOriginalOutput(t *testing.T, original *termtrace.OriginalSession, text string) {
	t.Helper()

	_, err := original.Session().
		Output().
		WaitFor(within(t, 30*time.Second), []byte(text), 20*time.Second)
	if err != nil {
		t.Fatalf("%s did not appear: %v", text, err)
	}
}

func procStat(pid int) (string, bool) {
	data, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/stat")
	if err != nil {
		return "", false
	}

	end := bytes.LastIndexByte(data, ')')
	if end < 0 || end+2 > len(data) {
		return "", false
	}

	return string(data[end+2:]), true
}

func parentOf(pid int) int {
	stat, ok := procStat(pid)
	if !ok {
		return 0
	}

	fields := strings.Fields(stat)
	if len(fields) < 2 {
		return 0
	}

	parent, _ := strconv.Atoi(fields[1])

	return parent
}

func children(t *testing.T, pid int) []int {
	t.Helper()

	entries, err := os.ReadDir("/proc")
	fatal(t, err)

	var result []int

	for _, entry := range entries {
		child, err := strconv.Atoi(entry.Name())
		if err == nil && parentOf(child) == pid {
			result = append(result, child)
		}
	}

	return result
}

func alive(pid int) bool {
	stat, ok := procStat(pid)
	return ok && !strings.HasPrefix(stat, "Z")
}

func waitDead(t *testing.T, pid int, timeout time.Duration) bool {
	t.Helper()

	for deadline := time.Now().Add(timeout); alive(pid); {
		if time.Now().After(deadline) {
			return false
		}

		time.Sleep(50 * time.Millisecond)
	}

	return true
}

func pidOf(value termtrace.DecimalU64) int {
	return int(value)
}

func sidecarOf(original *termtrace.OriginalSession) string {
	return string(original.Path()) + ".original-capture.json"
}

// rewriteStatus replaces the sidecar atomically, as the supervisor publishes it.
func rewriteStatus(t *testing.T, sidecar string, status []byte) {
	t.Helper()

	staged := sidecar + ".rewrite"

	file, err := os.OpenFile(staged, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	fatal(t, err)
	_, err = file.Write(status)
	fatal(t, errors.Join(err, file.Close()))
	fatal(t, os.Rename(staged, sidecar))
}

// replaced returns document with the member at path set to value, keeping every other value
// byte for byte. A string path element names an object member, an int an array index.
func replaced(t *testing.T, document jsontext.Value, value any, path ...any) jsontext.Value {
	t.Helper()

	if len(path) == 0 {
		data, err := json.Marshal(value)
		fatal(t, err)

		return data
	}

	switch key := path[0].(type) {
	case string:
		var object map[string]jsontext.Value
		fatal(t, json.Unmarshal(document, &object))

		if _, ok := object[key]; !ok {
			t.Fatalf("document has no member %q", key)
		}

		object[key] = replaced(t, object[key], value, path[1:]...)

		data, err := json.Marshal(object)
		fatal(t, err)

		return data
	case int:
		var array []jsontext.Value
		fatal(t, json.Unmarshal(document, &array))

		if key >= len(array) {
			t.Fatalf("array has no index %d", key)
		}

		array[key] = replaced(t, array[key], value, path[1:]...)

		data, err := json.Marshal(array)
		fatal(t, err)

		return data
	default:
		t.Fatalf("path element %v is neither a member nor an index", key)
		return nil
	}
}

func captureFailure(
	t *testing.T,
	err error,
) (*termtrace.ErrorData, termtrace.OriginalCaptureFacts) {
	t.Helper()

	failure := termtraceError(t, err)
	if failure.Facts == nil {
		t.Fatalf("capture failure has no facts: %v", err)
	}

	management, ok := failure.Facts.Management.(termtrace.ManagementFailureOriginalCapture)
	if !ok {
		t.Fatalf("management = %T, want original_capture: %v", failure.Facts.Management, err)
	}

	return failure.Facts, management.Facts
}

func TestOriginalResizesInsideTheCanvasAndPublishes(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 120*time.Second)
	original := recordOriginal(t, client, []string{"/bin/sh", "-c", winchLoop}, originalOptions())

	status, err := original.CaptureStatus(ctx)
	fatal(t, err)

	if status.State != termtrace.OriginalCaptureStateLive {
		t.Errorf("capture state = %s, want live", status.State)
	}

	waitForOriginalOutput(t, original, "ready")

	_, err = original.Session().Resize(ctx, 100, 30, termtrace.MutationOptions{})
	fatal(t, err)
	waitForOriginalOutput(t, original, "winch 30 100")

	_, err = original.Session().Resize(ctx, 400, 200, termtrace.MutationOptions{})
	if err == nil {
		t.Fatal("a resize past the native canvas succeeded")
	}

	if got := errorFacts(t, err); got.Code != "exceeds_native_canvas" ||
		got.Effect["kind"] != "not_applied" {
		t.Errorf("oversized resize = %+v", got)
	}

	result, err := original.Finish(ctx, termtrace.FinishOptions{Timeout: 60 * time.Second})
	fatal(t, err)

	if _, ok := result.Capture.Provenance.(termtrace.ArtifactProvenanceOriginalExecution); !ok {
		t.Errorf("provenance = %T, want original_execution", result.Capture.Provenance)
	}

	if result.Capture.Source.Boundary.OutputBytes == 0 {
		t.Error("the capture boundary holds no output")
	}

	if result.Completion.SessionID != original.ID() || result.CanonicalSuccess {
		t.Errorf("completion session %s, canonical success %v",
			result.Completion.SessionID, result.CanonicalSuccess)
	}

	active, err := original.IsActive(ctx)
	fatal(t, err)

	if active {
		t.Error("the recording is still active after finish")
	}
}

func TestOriginalFailingApplicationStillPublishes(t *testing.T) {
	client, _ := newClient(t)
	original := recordOriginal(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo failing; exit 7"},
		originalOptions(),
	)

	result, err := original.WaitFinished(within(t, 90*time.Second), 60*time.Second)
	fatal(t, err)

	if result.Completion.ExitCode == nil || *result.Completion.ExitCode != 7 ||
		result.CanonicalSuccess {
		t.Errorf("exit code %v, canonical success %v",
			result.Completion.ExitCode, result.CanonicalSuccess)
	}
}

func TestOriginalKilledEncoderFailsWithCanonicalCompletion(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 120*time.Second)
	original := recordOriginal(t, client, []string{"/bin/sh", "-c", winchLoop}, originalOptions())

	status, err := original.CaptureStatus(ctx)
	fatal(t, err)

	supervisor := pidOf(status.SupervisorPID)

	var encoders []int

	for _, child := range children(t, supervisor) {
		for _, grandchild := range children(t, child) {
			comm, err := os.ReadFile("/proc/" + strconv.Itoa(grandchild) + "/comm")
			if err == nil && strings.HasPrefix(string(comm), "ffmpeg") {
				encoders = append(encoders, grandchild)
			}
		}
	}

	if len(encoders) != 1 {
		t.Fatalf("encoders = %v, want one", encoders)
	}

	fatal(t, syscall.Kill(encoders[0], syscall.SIGKILL))

	_, err = original.Finish(ctx, termtrace.FinishOptions{Timeout: 60 * time.Second})
	if err == nil {
		t.Fatal("finish after the encoder died succeeded")
	}

	data, facts := captureFailure(t, err)
	if data.Kind != "original_capture_failed" || data.Code != "acquisition_failed" ||
		data.Scope != termtrace.FailureScopeManagement {
		t.Errorf("failure = %+v", data)
	}

	if facts.Completion == nil || facts.Session != original.Path() {
		t.Errorf("completion %v, session %q", facts.Completion, facts.Session)
	}

	if alive(supervisor) {
		t.Error("the supervisor outlived the failed capture")
	}
}

func TestOriginalCanceledStartupTearsTheCaptureDown(t *testing.T) {
	client, root := newClient(t)
	sessions := filepath.Join(root, "sessions")
	ctx, cancel := context.WithCancel(t.Context())

	defer cancel()

	type startup struct {
		original *termtrace.OriginalSession
		err      error
	}

	started := make(chan startup, 1)

	go func() {
		original, err := client.RecordOriginal(
			ctx,
			[]string{"/bin/sh", "-c", "sleep 30"},
			originalOptions(),
		)
		started <- startup{original: original, err: err}
	}()

	var sidecar string

	for deadline := time.Now().Add(30 * time.Second); sidecar == ""; {
		if time.Now().After(deadline) {
			cancel()
			t.Fatal("acquisition never started")
		}

		matches, err := filepath.Glob(filepath.Join(sessions, "*.original-capture.json"))
		fatal(t, err)

		if len(matches) > 0 {
			sidecar = matches[0]
		} else {
			time.Sleep(10 * time.Millisecond)
		}
	}

	cancel()

	if outcome := <-started; outcome.err == nil {
		owned(t, outcome.original)
		t.Fatal("startup succeeded despite the cancel")
	} else if !errors.Is(outcome.err, context.Canceled) {
		t.Fatalf("canceled startup = %v", outcome.err)
	}

	var status termtrace.OriginalCaptureStatus

	for deadline := time.Now().Add(30 * time.Second); ; {
		data, err := os.ReadFile(sidecar)
		fatal(t, err)
		fatal(t, termtrace.DecodeJSON(data, &status))

		if status.State == termtrace.OriginalCaptureStateCancelled || time.Now().After(deadline) {
			break
		}

		time.Sleep(50 * time.Millisecond)
	}

	if status.State != termtrace.OriginalCaptureStateCancelled || status.Failure == nil ||
		status.Failure.Code != termtrace.OriginalCaptureFailureCancelled {
		t.Errorf("capture after cancel = %s, failure %+v", status.State, status.Failure)
	}

	if !waitDead(t, pidOf(status.SupervisorPID), 30*time.Second) {
		t.Error("the supervisor outlived the canceled startup")
	}
}

func TestOriginalConflictingGeometryIsRejected(t *testing.T) {
	client, root := newClient(t)

	_, err := client.RecordOriginal(
		within(t, 30*time.Second),
		[]string{"/bin/true"},
		termtrace.OriginalLaunchOptions{
			Grid:     &termtrace.CellGrid{Cols: 80, Rows: 24},
			Viewport: &termtrace.PixelCanvas{Width: 800, Height: 600},
		},
	)
	if err == nil {
		t.Fatal("conflicting geometry was accepted")
	}

	if kind := termtraceError(t, err).Kind(); kind != "invalid_operation" {
		t.Errorf("kind = %s, want invalid_operation", kind)
	}

	if entries, err := os.ReadDir(filepath.Join(root, "sessions")); err == nil && len(entries) > 0 {
		t.Errorf("a rejected recording left %d entries", len(entries))
	}
}

func TestOriginalDetachedRecordingAttachesForControlAndFinish(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 120*time.Second)
	state := filepath.Join(root, "state")
	options := originalOptions()
	options.Alias = new("go-attach")
	options.StateRoot = new(state)

	started := recordOriginal(t, client, []string{"/bin/sh", "-c", winchLoop}, options)
	fatal(t, started.Detach(ctx))

	entry, err := client.Registry(state).Resolve(ctx, "go-attach")
	fatal(t, err)

	if entry == nil || entry.SessionID != started.ID() {
		t.Fatalf("alias = %+v, want %s", entry, started.ID())
	}

	attached := attachOriginal(t, client, string(started.Path()))
	if attached.ID() != started.ID() {
		t.Errorf("attached session %s, want %s", attached.ID(), started.ID())
	}

	status, err := attached.CaptureStatus(ctx)
	fatal(t, err)

	if status.State != termtrace.OriginalCaptureStateLive {
		t.Errorf("capture state = %s, want live", status.State)
	}

	waitForOriginalOutput(t, attached, "ready")
	_, err = attached.Session().Resize(ctx, 100, 30, termtrace.MutationOptions{})
	fatal(t, err)
	waitForOriginalOutput(t, attached, "winch 30 100")

	second := attachOriginal(t, client, string(attached.Path()))
	if second.Path() != attached.Path() {
		t.Errorf("second attach path %q, want %q", second.Path(), attached.Path())
	}

	result, err := attached.Finish(ctx, termtrace.FinishOptions{Timeout: 60 * time.Second})
	fatal(t, err)

	if result.Completion.SessionID != started.ID() {
		t.Errorf("completion session %s, want %s", result.Completion.SessionID, started.ID())
	}

	if _, ok := result.Capture.Provenance.(termtrace.ArtifactProvenanceOriginalExecution); !ok {
		t.Errorf("provenance = %T, want original_execution", result.Capture.Provenance)
	}

	published, err := second.CaptureStatus(ctx)
	fatal(t, err)

	if published.State != termtrace.OriginalCaptureStatePublished {
		t.Errorf("second handle capture state = %s, want published", published.State)
	}
}

const controlLoop = "(trap 'echo winch $(stty size </dev/tty)' WINCH; while :; do sleep 0.05; done) & " +
	"echo ready; while read line; do echo got $line; done"

func TestOriginalNonUTF8PathAttachesControlsAndFinishes(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 180*time.Second)
	evidence := root + "/evidence-\xff"
	session := evidence + "/session"
	fatal(t, os.Mkdir(evidence, 0o700))

	command := exec.CommandContext(
		ctx,
		binary(t),
		"record", "--original-native", "--original-grid", "80x24",
		"--original-canvas", "1280x720", "-d", "go-bytes", "--out", session,
		"--", "/bin/sh", "-c", controlLoop,
	)

	command.Env = append(os.Environ(), "TERMTRACE_STATE_DIR="+filepath.Join(root, "state"))

	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("record: %v: %s", err, output)
	}

	exact := attachOriginal(t, client, session)
	if string(exact.Path()) != session {
		t.Errorf("attached path %q, want %q", exact.Path(), session)
	}

	status, err := exact.CaptureStatus(ctx)
	fatal(t, err)

	if status.State != termtrace.OriginalCaptureStateLive {
		t.Errorf("capture state = %s, want live", status.State)
	}

	control := exact.Session()
	if string(control.Path()) != session {
		t.Errorf("control path %q, want %q", control.Path(), session)
	}

	presented, err := control.Screen().WaitForText(ctx, "ready", 20*time.Second)
	fatal(t, err)

	acted, err := control.Actions().
		IfState(presented.Hash).
		SendText("hello").
		SendKey("Enter").
		UntilScreen("got hello", 20*time.Second).
		Run(ctx)
	fatal(t, err)

	screen, ok := acted.Postcondition.(termtrace.ActionPostconditionResultScreen)
	if !ok || !screen.Matched || screen.Frame == nil ||
		!strings.Contains(screen.Frame.Text(), "got hello") {
		t.Errorf("action postcondition = %+v", acted.Postcondition)
	}

	_, err = control.Resize(ctx, 100, 30, termtrace.MutationOptions{})
	fatal(t, err)
	_, err = control.Screen().WaitForText(ctx, "winch 30 100", 20*time.Second)
	fatal(t, err)

	second := attachOriginal(t, client, session)
	if second.ID() != exact.ID() {
		t.Errorf("second attach session %s, want %s", second.ID(), exact.ID())
	}

	result, err := exact.Finish(ctx, termtrace.FinishOptions{Timeout: 60 * time.Second})
	fatal(t, err)

	if !bytes.HasPrefix(result.Capture.DirectoryBytes, []byte(session+"/")) {
		t.Errorf("capture directory %q is outside %q", result.Capture.DirectoryBytes, session)
	}

	published, err := second.CaptureStatus(ctx)
	fatal(t, err)

	if published.State != termtrace.OriginalCaptureStatePublished {
		t.Errorf("second handle capture state = %s, want published", published.State)
	}
}

func TestOriginalCaptureStatusExactCountersAndMalformedDecimals(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 120*time.Second)
	original := recordOriginal(t, client, []string{"/bin/sh", "-c", "echo done"}, originalOptions())

	_, err := original.WaitFinished(ctx, 60*time.Second)
	fatal(t, err)

	sidecar := sidecarOf(original)
	published, err := os.ReadFile(sidecar)
	fatal(t, err)

	const (
		u64Max  = "18446744073709551615"
		unsafe  = "9007199254740993"
		maximum = termtrace.DecimalU64(18446744073709551615)
		beyond  = termtrace.DecimalU64(9007199254740993)
	)

	wide := replaced(t, published, u64Max, "supervisor_pid")
	wide = replaced(t, wide, unsafe, "supervisor_start_ticks")
	wide = replaced(t, wide, u64Max, "result", "source", "boundary", "output_bytes")
	wide = replaced(t, wide, unsafe, "result", "members", 0, "byte_length")
	rewriteStatus(t, sidecar, wide)

	status, err := original.CaptureStatus(ctx)
	fatal(t, err)

	if status.SupervisorPID != maximum || status.SupervisorStartTicks != beyond ||
		status.Result == nil || status.Result.Source.Boundary.OutputBytes != maximum ||
		len(status.Result.Members) == 0 || status.Result.Members[0].ByteLength != beyond ||
		status.Result.Members[0].SchemaType != nil {
		t.Errorf("wide status = %+v", status)
	}

	failed := replaced(t, published, "failed", "state")
	failed = replaced(t, failed, nil, "result")
	failed = replaced(t, failed, map[string]any{
		"code":      "recording_failed",
		"reason":    u64Max,
		"surviving": []any{},
		"cleanup":   nil,
	}, "failure")
	rewriteStatus(t, sidecar, failed)

	status, err = original.CaptureStatus(ctx)
	fatal(t, err)

	if status.Result != nil || status.Failure == nil || status.Failure.Reason != u64Max ||
		status.Failure.Cleanup != nil {
		t.Errorf("failed status = result %+v, failure %+v", status.Result, status.Failure)
	}

	for _, bad := range []any{"01", "-1", "18446744073709551616", 7} {
		rewriteStatus(t, sidecar, replaced(t, published, bad, "supervisor_pid"))

		if _, err := original.CaptureStatus(ctx); err == nil {
			t.Errorf("supervisor_pid %v decoded", bad)
		}
	}
}

func TestOriginalSwappedSidecarIsACaptureFailure(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 180*time.Second)
	first := recordOriginal(t, client, []string{"/bin/sh", "-c", "echo first"}, originalOptions())
	_, err := first.WaitFinished(ctx, 60*time.Second)
	fatal(t, err)

	second := recordOriginal(t, client, []string{"/bin/sh", "-c", "echo second"}, originalOptions())
	_, err = second.WaitFinished(ctx, 60*time.Second)
	fatal(t, err)

	other, err := os.ReadFile(sidecarOf(first))
	fatal(t, err)
	own, err := os.ReadFile(sidecarOf(second))
	fatal(t, err)

	attached := attachOriginal(t, client, string(second.Path()))
	rewriteStatus(t, sidecarOf(second), other)

	if _, err := attached.CaptureStatus(ctx); err == nil {
		t.Error("a swapped sidecar decoded as this capture's status")
	}

	_, err = attached.WaitFinished(ctx, 60*time.Second)
	if err == nil {
		t.Fatal("waiting on a swapped sidecar succeeded")
	}

	data, facts := captureFailure(t, err)
	if data.Kind != "original_capture_failed" || data.Code != "publication_failed" ||
		facts.Completion == nil || facts.Completion.SessionID != second.ID() {
		t.Errorf("swapped sidecar failure = %+v, completion %+v", data, facts.Completion)
	}

	var otherStatus struct {
		Result struct {
			DirectoryBytes jsontext.Value `json:"directory_bytes"`
		} `json:"result"`
	}

	fatal(t, json.Unmarshal(other, &otherStatus))
	redirected := replaced(t, own, otherStatus.Result.DirectoryBytes, "result", "directory_bytes")

	rewriteStatus(t, sidecarOf(second), own)
	moved := attachOriginal(t, client, string(second.Path()))
	rewriteStatus(t, sidecarOf(second), redirected)

	_, err = moved.WaitFinished(ctx, 60*time.Second)
	if err == nil {
		t.Fatal("waiting on a redirected capture succeeded")
	}

	if data, _ := captureFailure(t, err); data.Code != "publication_failed" {
		t.Errorf("redirected capture code = %s, want publication_failed", data.Code)
	}

	rewriteStatus(t, sidecarOf(second), own)
	restored := attachOriginal(t, client, string(second.Path()))

	result, err := restored.WaitFinished(ctx, 60*time.Second)
	fatal(t, err)

	if result.Capture.Source.SessionID != second.ID() {
		t.Errorf(
			"restored capture session %s, want %s",
			result.Capture.Source.SessionID,
			second.ID(),
		)
	}
}

func TestOriginalOwnerStopsApplicationAfterSupervisorLoss(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 120*time.Second)
	original := recordOriginal(
		t,
		client,
		[]string{"/bin/sh", "-c", "trap '' HUP TERM; echo stubborn; while :; do sleep 0.05; done"},
		originalOptions(),
	)

	capture, err := original.CaptureStatus(ctx)
	fatal(t, err)

	status, err := original.Session().Status(ctx)
	fatal(t, err)

	live, ok := status.(termtrace.SessionStatusLive)
	if !ok || live.PID == nil {
		t.Fatalf("session status = %+v, want live with a pid", status)
	}

	application := *live.PID
	for _, pid := range []int{pidOf(capture.SupervisorPID), parentOf(application)} {
		fatal(t, syscall.Kill(pid, syscall.SIGKILL))
	}

	if !waitDead(t, application, 15*time.Second) {
		t.Fatal("the application outlived its supervisor")
	}

	_, err = original.WaitFinished(ctx, 60*time.Second)
	if err == nil {
		t.Fatal("waiting after supervisor loss succeeded")
	}

	if code := termtraceError(t, err).Facts; code == nil || code.Code != "supervisor_abandoned" {
		t.Errorf("supervisor loss = %v", err)
	}
}
