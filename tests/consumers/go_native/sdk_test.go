package consumer

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

func uuid4() string {
	var id [16]byte

	_, _ = rand.Read(id[:])
	id[6] = id[6]&0x0f | 0x40
	id[8] = id[8]&0x3f | 0x80

	return formatUUID(id)
}

func uuid5(namespace string, name string) string {
	space, _ := hex.DecodeString(strings.ReplaceAll(namespace, "-", ""))
	digest := sha1.Sum(append(space, name...))

	var id [16]byte
	copy(id[:], digest[:16])
	id[6] = id[6]&0x0f | 0x50
	id[8] = id[8]&0x3f | 0x80

	return formatUUID(id)
}

func formatUUID(id [16]byte) string {
	text := hex.EncodeToString(id[:])
	return text[0:8] + "-" + text[8:12] + "-" + text[12:16] + "-" + text[16:20] + "-" + text[20:]
}

func TestRecordingLifecycle(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo GO_CONSUMER_OK; sleep 30"},
		termtrace.LaunchOptions{Rows: new(24), Cols: new(80)},
	)
	if active, err := recording.IsActive(ctx); err != nil || !active {
		t.Fatalf("active = %v, %v", active, err)
	}

	if recording.ID() == "" {
		t.Error("empty session id")
	}

	state, err := recording.Screen().WaitForText(ctx, "GO_CONSUMER_OK", 5*time.Second)
	fatal(t, err)

	if !strings.Contains(state.Text, "GO_CONSUMER_OK") || state.Frame.StateHash != state.Hash {
		t.Errorf("state = %q %s", state.Text, state.Hash)
	}

	fatal(t, recording.Stop(ctx))

	if active, err := recording.IsActive(ctx); err != nil || active {
		t.Errorf("active after stop = %v, %v", active, err)
	}

	if _, err := recording.Screen().
		Text(ctx); err == nil ||
		!strings.Contains(err.Error(), "stopped or detached") {
		t.Errorf("screen of a stopped recording = %v", err)
	}

	status, err := recording.Status(ctx)
	fatal(t, err)

	if _, ok := status.(termtrace.SessionStatusCompleted); !ok {
		t.Errorf("status after stop = %T", status)
	}
}

func TestScreenFrameGeometryAndText(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo FRAME_GEOMETRY_OK; sleep 30"},
		termtrace.LaunchOptions{Rows: new(32), Cols: new(96)},
	)
	_, err := recording.Screen().WaitForText(ctx, "FRAME_GEOMETRY_OK", 5*time.Second)
	fatal(t, err)
	frame, err := recording.Screen().Frame(ctx, termtrace.Projection{})
	fatal(t, err)

	if frame.Geometry != (termtrace.ScreenGeometry{Rows: 32, Cols: 96}) ||
		!strings.Contains(frame.Text(), "FRAME_GEOMETRY_OK") {
		t.Errorf("frame = %+v %q", frame.Geometry, frame.Text())
	}

	text, err := recording.Screen().Text(ctx)
	fatal(t, err)

	if !strings.Contains(text, "FRAME_GEOMETRY_OK") {
		t.Errorf("text = %q", text)
	}
}

func TestFailuresCarryTypedKindCodeEffectAndRetry(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(t, client, []string{"/bin/sh", "-c", "sleep 10"}, termtrace.LaunchOptions{})

	_, err := recording.Screen().
		WaitForText(t.Context(), "ABSENT_PATTERN_123", 200*time.Millisecond)
	got := errorFacts(t, err)

	want := facts{
		Kind:   "wait_unsatisfied",
		Code:   "wait_unsatisfied",
		Effect: map[string]any{"kind": "not_applied"},
		Retry:  "new_request",
	}
	if !sameFacts(got, want) || !strings.Contains(err.Error(), "timed out waiting for pattern") {
		t.Errorf("wait failure = %+v: %v", got, err)
	}

	_, err = recording.Actions().IfState(strings.Repeat("0", 64)).SendKey("ctrl+shift+F5").Run(ctx)
	got = errorFacts(t, err)

	want = facts{
		Kind:   "stale_state",
		Code:   "stale_observation",
		Effect: map[string]any{"kind": "not_applied"},
		Retry:  "new_request",
	}
	if !sameFacts(got, want) {
		t.Errorf("stale failure = %+v", got)
	}
}

func TestRunSealsTheRequestAndReplaysItWithTheSameIdentity(t *testing.T) {
	fixture := interactionFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := withEcho(t, client, fixture)

	_, err := recording.Actions().WithRequestID("not-a-uuid").SendText("x").Run(ctx)
	if got := errorFacts(t, err); got.Kind != "invalid_operation" {
		t.Errorf("invalid request id = %+v", got)
	}

	chosen := uuid4()

	builder := recording.Actions().WithRequestID(chosen)
	if builder.RequestID() != chosen {
		t.Fatalf("request id = %s", builder.RequestID())
	}

	builder.SendText("once").
		SendKey("Enter").
		UntilScreenWhere(termtrace.ScreenPredicateContains{Text: "GOT:once"}, 5*time.Second)
	result, err := builder.Run(ctx)
	fatal(t, err)

	if !builder.Sealed() || result.RequestID != chosen || result.Input.SequenceID != chosen ||
		!result.Matched() {
		t.Errorf("result = %+v", result)
	}

	if frame := result.Frame(); frame == nil || !strings.Contains(frame.Text(), "GOT:once") ||
		result.StateHash() != frame.StateHash {
		t.Errorf("result frame = %v", frame)
	}

	builder.SendText("more")

	_, err = builder.Run(ctx)
	if got := errorFacts(t, err); !sameFacts(got, fixture.Errors["sealed"]) {
		t.Errorf("changing a sealed action = %+v", got)
	}

	retry, err := builder.Run(ctx)
	fatal(t, err)

	if !sameJSON(t, retry.Input, result.Input) {
		t.Errorf("retry input = %+v, want %+v", retry.Input, result.Input)
	}

	text, err := recording.Screen().Text(ctx)
	fatal(t, err)

	if count := strings.Count(text, "GOT:once"); count != 1 {
		t.Errorf("GOT:once appears %d times", count)
	}

	data, err := json.Marshal(result.ActOutcome)
	fatal(t, err)

	var restored termtrace.ActOutcome
	fatal(t, termtrace.DecodeJSON(data, &restored))
	again, err := json.Marshal(restored)
	fatal(t, err)

	if !bytes.Equal(data, again) {
		t.Errorf("outcome does not round trip:\n%s\n%s", data, again)
	}
}

func TestMutationFailuresMatchTheSharedFixture(t *testing.T) {
	fixture := interactionFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := withEcho(t, client, fixture)

	stale := recording.Actions().IfState(strings.Repeat("0", 64)).SendText("stale\n")

	_, err := stale.Run(ctx)
	if got := errorFacts(t, err); !sameFacts(got, fixture.Errors["stale"]) {
		t.Errorf("stale = %+v", got)
	}

	failure := termtraceError(t, err)
	if failure.Facts.RequestID == nil || *failure.Facts.RequestID != stale.RequestID() ||
		failure.Facts.Control == nil {
		t.Errorf("stale facts = %+v", failure.Facts)
	}

	first := recording.Actions().SendText("first\n")
	_, err = first.Run(ctx)
	fatal(t, err)

	_, err = recording.Actions().WithRequestID(first.RequestID()).SendText("other\n").Run(ctx)
	if got := errorFacts(t, err); !sameFacts(got, fixture.Errors["collision"]) {
		t.Errorf("collision = %+v", got)
	}

	_, err = recording.Pointer().Click(ctx, 0, 0, termtrace.PointerOptions{Strict: true})
	if got := errorFacts(
		t,
		err,
	); !sameFacts(got, fixture.Errors["strict_pointer"]) ||
		termtraceError(t, err).Facts.RequestID == nil {
		t.Errorf("strict pointer = %+v", got)
	}

	_, err = recording.Actions().
		SendText("late\n").
		UntilScreen("NEVER", 200*time.Millisecond).
		Run(ctx)
	if got := errorFacts(t, err); !sameFacts(got, fixture.Errors["incomplete"]) {
		t.Errorf("incomplete = %+v", got)
	}

	if result := termtraceError(
		t,
		err,
	).Facts.ActionResult; result == nil || result.Input.AtomCount != 1 ||
		(termtrace.ActionResult{ActOutcome: *result}).Matched() {
		t.Errorf("incomplete result = %+v", result)
	}

	state, err := recording.Screen().WaitForText(ctx, "GOT:late", 5*time.Second)
	fatal(t, err)

	if !strings.Contains(state.Text, "GOT:first") || strings.Contains(state.Text, "GOT:stale") ||
		strings.Contains(state.Text, "GOT:other") {
		t.Errorf("screen = %q", state.Text)
	}
}

func TestSendReportsDerivedStepIdentitiesAndPartialFailures(t *testing.T) {
	fixture := interactionFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := withEcho(t, client, fixture)
	stepID := func(send string, index int) string {
		name := strings.NewReplacer("{send}", send, "{index}", strconv.Itoa(index)).
			Replace(fixture.SendStepName)

		return uuid5(fixture.SendStepNamespace, name)
	}

	sendID := uuid4()
	result, err := recording.Send(ctx, termtrace.SendSpec{
		Steps: []termtrace.SendStepSpec{
			termtrace.SendStepSpecMark{Label: "before"},
			termtrace.SendStepSpecInput{
				Atoms: []termtrace.AtomSpec{
					termtrace.AtomSpecText{Text: "sent"},
					termtrace.AtomSpecKey{Key: "Enter"},
				},
			},
		},
		RequestID: new(sendID),
		Until: termtrace.ConditionSpecScreen{
			Predicate: termtrace.ScreenPredicateContains{Text: "GOT:sent"},
		},
	})
	fatal(t, err)

	last, ok := result.Last.(termtrace.SendLastAction)
	if result.RequestID != sendID || !ok || last.Step != 1 || last.RequestID != stepID(sendID, 1) {
		t.Errorf("send = %+v", result)
	}

	failingID := uuid4()
	_, err = recording.Send(ctx, termtrace.SendSpec{
		Steps: []termtrace.SendStepSpec{
			termtrace.SendStepSpecMark{Label: "applied"},
			termtrace.SendStepSpecPointer{
				Column:    0,
				Row:       0,
				Operation: termtrace.PointerOperationClick{Button: termtrace.MouseButtonLeft},
				Strict:    true,
			},
		},
		RequestID: new(failingID),
	})

	failure := termtraceError(t, err)
	if got := errorFacts(t, err); !sameFacts(got, fixture.Errors["strict_pointer"]) {
		t.Errorf("failed send = %+v", got)
	}

	if failure.Facts.Step == nil || *failure.Facts.Step != 1 ||
		failure.Facts.SendRequestID == nil ||
		*failure.Facts.SendRequestID != failingID ||
		failure.Facts.RequestID == nil ||
		*failure.Facts.RequestID != stepID(failingID, 1) {
		t.Errorf("failed send facts = %+v", failure.Facts)
	}
}

func TestRawBytesAndWideIntegersStayExact(t *testing.T) {
	fixture := interactionFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(t, client, fixture.RawCommand, termtrace.LaunchOptions{})
	_, err := recording.Screen().WaitForText(ctx, fixture.ReadyText, 5*time.Second)
	fatal(t, err)
	raw, err := hex.DecodeString(fixture.RawBytesHex)
	fatal(t, err)
	result, err := recording.Actions().SendBytes(raw).Run(ctx)
	fatal(t, err)

	if result.Input.BytesWritten != uint64(len(raw)) {
		t.Errorf("bytes written = %d", result.Input.BytesWritten)
	}

	_, err = recording.Screen().WaitForText(ctx, fixture.RawBytesEcho, 5*time.Second)
	fatal(t, err)

	report, err := json.Marshal(result.ActOutcome)
	fatal(t, err)

	written := fmt.Sprintf(`"bytes_written":%d`, len(raw))
	if !bytes.Contains(report, []byte(written)) {
		t.Fatalf("report lacks %s: %s", written, report)
	}

	var wide termtrace.ActOutcome
	fatal(
		t,
		termtrace.DecodeJSON(
			bytes.Replace(report, []byte(written), []byte(`"bytes_written":9007199254740993`), 1),
			&wide,
		),
	)

	if wide.Input.BytesWritten != 9007199254740993 {
		t.Errorf("wide = %d", wide.Input.BytesWritten)
	}

	for _, malformed := range []string{"-1", "1.5", "18446744073709551616"} {
		var rejected termtrace.ActOutcome
		if err := termtrace.DecodeJSON(
			bytes.Replace(report, []byte(written), []byte(`"bytes_written":`+malformed), 1),
			&rejected,
		); err == nil {
			t.Errorf("accepted bytes_written %s", malformed)
		}
	}
}

func TestTypedPointerOperationsReportTheirOutcomes(t *testing.T) {
	fixture := interactionFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(t, client, fixture.MouseCommand, termtrace.LaunchOptions{})
	_, err := recording.Screen().WaitForText(ctx, fixture.ReadyText, 5*time.Second)
	fatal(t, err)

	pointer := recording.Pointer()

	multi, err := pointer.MultiClick(ctx, 3, 0, 2, termtrace.PointerOptions{Strict: true})
	fatal(t, err)

	applied, ok := multi.(termtrace.PointerOutcomeApplied)
	if !ok || strings.Count(string(applied.Bytes), "\x1b[<") != 4 ||
		applied.BytesWritten != uint64(len(applied.Bytes)) {
		t.Errorf("multi click = %+v", multi)
	}

	drag, err := pointer.Drag(ctx, 1, 0, 5, 0, termtrace.DragOptions{Steps: 3, Strict: true})
	fatal(t, err)

	if dragged, ok := drag.(termtrace.PointerOutcomeApplied); !ok ||
		!strings.Contains(string(dragged.Bytes), "<32;") {
		t.Errorf("drag = %+v", drag)
	}

	activated, err := pointer.Activate(
		ctx,
		fixture.ActivationText,
		termtrace.ActivateOptions{Unique: true, Strict: true},
	)
	fatal(t, err)

	if hit, ok := activated.(termtrace.PointerOutcomeActivated); !ok ||
		hit.Text != fixture.ActivationText ||
		hit.StartColumn != 0 ||
		hit.Row != 0 {
		t.Errorf("activate = %+v", activated)
	}

	echo := withEcho(t, client, fixture)
	noop, err := echo.Pointer().Click(ctx, 0, 0, termtrace.PointerOptions{})
	fatal(t, err)

	if skipped, ok := noop.(termtrace.PointerOutcomeNoop); !ok ||
		skipped.Reason != "pointer_reporting_disabled" {
		t.Errorf("click without reporting = %+v", noop)
	}
}

func TestSessionControlsAndStatus(t *testing.T) {
	fixture := interactionFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := withEcho(t, client, fixture)

	status, err := recording.Status(ctx)
	fatal(t, err)

	if _, ok := status.(termtrace.SessionStatusLive); !ok {
		t.Errorf("status = %T", status)
	}

	resized, err := recording.Resize(ctx, 100, 30, termtrace.MutationOptions{})
	fatal(t, err)

	if resized.RequestID == "" {
		t.Error("resize has no request id")
	}

	frame, err := recording.Screen().Frame(ctx, termtrace.Projection{})
	fatal(t, err)

	if frame.Geometry != (termtrace.ScreenGeometry{Cols: 100, Rows: 30}) {
		t.Errorf("geometry after resize = %+v", frame.Geometry)
	}

	_, err = recording.Signal(ctx, termtrace.ProcessSignalInterrupt, termtrace.MutationOptions{})
	fatal(t, err)
	_, err = recording.Screen().WaitForText(ctx, "SIGNAL:INT", 5*time.Second)
	fatal(t, err)
	marked, err := recording.Mark(ctx, "checkpoint", termtrace.MutationOptions{})
	fatal(t, err)

	if marked.Label != "checkpoint" || marked.EventSeq == 0 {
		t.Errorf("mark = %+v", marked)
	}

	stopID := uuid4()
	stopped, err := recording.RequestStop(
		ctx,
		termtrace.StopOptions{RequestID: stopID, Wait: 5 * time.Second},
	)
	fatal(t, err)

	if completed, ok := stopped.(termtrace.StopReportCompleted); !ok ||
		completed.RequestID != stopID {
		t.Errorf("stop = %+v", stopped)
	}

	if active, err := recording.IsActive(ctx); err != nil || active {
		t.Errorf("active after stop = %v, %v", active, err)
	}

	status, err = recording.Status(ctx)
	fatal(t, err)

	if _, ok := status.(termtrace.SessionStatusCompleted); !ok {
		t.Errorf("status after stop = %T", status)
	}
}

func TestLaunchEnvironmentIsIsolatedAndExact(t *testing.T) {
	fixture := interactionFixture(t)
	t.Setenv("TT_COPY", fixture.CopyValue)
	t.Setenv("TT_SECRET", fixture.SecretValue)
	client, dir := newClient(t)
	ctx := within(t, 30*time.Second)
	set, err := hex.DecodeString(fixture.SetBytesHex)
	fatal(t, err)
	recording := launch(t, client, fixture.EnvironmentCommand, termtrace.LaunchOptions{
		Environment: &termtrace.EnvironmentSpec{
			Base: termtrace.EnvironmentBaseClear,
			Set:  map[string]termtrace.Base64Bytes{"TT_SET": set},
			Copy: []string{"TT_COPY"},
		},
	})
	state, err := recording.Screen().WaitForText(ctx, "DONE", 5*time.Second)
	fatal(t, err)

	wants := []string{
		"SET= " + fixture.SetBytesEcho,
		"COPY=" + fixture.CopyValue,
		"SECRET=absent",
		"TERM=",
	}
	for _, want := range wants {
		if !strings.Contains(state.Text, want) {
			t.Errorf("screen lacks %q: %q", want, state.Text)
		}
	}

	fatal(t, recording.Stop(ctx))
	fatal(t, filepath.WalkDir(dir, func(path string, entry os.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return err
		}

		data, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("read %s: %w", path, err)
		}

		if bytes.Contains(data, []byte(fixture.SecretValue)) {
			t.Errorf("%s holds the secret", path)
		}

		return nil
	}))
}

func TestLaunchLimitsEndTheSession(t *testing.T) {
	for _, limit := range []string{"max_duration", "idle_timeout"} {
		t.Run(limit, func(t *testing.T) {
			client, _ := newClient(t)
			ctx := within(t, 30*time.Second)

			options := termtrace.LaunchOptions{}
			if limit == "max_duration" {
				options.MaxDurationMs = new(uint64(400))
			} else {
				options.IdleTimeoutMs = new(uint64(400))
			}

			recording, err := client.Launch(
				ctx,
				[]string{"/bin/sh", "-c", "printf READY; sleep 30"},
				options,
			)
			fatal(t, err)
			completion, err := recording.WaitForCompletion(ctx, 10*time.Second)
			fatal(t, err)

			if completion.ResourceLimit == nil || string(completion.ResourceLimit.Kind) != limit ||
				completion.ResourceLimit.Limit != 400 {
				t.Errorf("resource limit = %+v", completion.ResourceLimit)
			}
		})
	}
}

func TestAbandoningARunKeepsTheAdmittedAction(t *testing.T) {
	fixture := interactionFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := withEcho(t, client, fixture)
	builder := recording.Actions().
		SendText("slow\n").
		UntilScreen("NEVER", 800*time.Millisecond).
		AllowIncomplete(true)

	_, err := builder.Run(within(t, 50*time.Millisecond))
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("abandoned run = %v", err)
	}
	// The server still applies the abandoned action; a retry is rejected until it settles.
	var retry termtrace.ActionResult
	for {
		retry, err = builder.Run(ctx)

		var failure *termtrace.Error
		if !errors.As(err, &failure) || failure.Facts == nil ||
			failure.Facts.Code != "action_busy" {
			break
		}

		time.Sleep(50 * time.Millisecond)
	}

	fatal(t, err)

	if retry.RequestID != builder.RequestID() || retry.Matched() {
		t.Errorf("retry = %+v", retry)
	}

	text, err := recording.Screen().Text(ctx)
	fatal(t, err)

	if count := strings.Count(text, "GOT:slow"); count != 1 {
		t.Errorf("GOT:slow appears %d times", count)
	}
}

type atomicAction struct {
	Command           []string `json:"command"`
	ReadyText         string   `json:"ready_text"`
	RequestID         string   `json:"request_id"`
	ExpectedText      string   `json:"expected_text"`
	ExpectedAtomCount int      `json:"expected_atom_count"`
	FrameDetail       string   `json:"frame_detail"`
	Input             []struct {
		Kind string `json:"kind"`
		Text string `json:"text"`
		Key  string `json:"key"`
	} `json:"input"`
	Predicate jsontext.Value `json:"predicate"`
	Region    struct {
		Top    int `json:"top"`
		Left   int `json:"left"`
		Width  int `json:"width"`
		Height int `json:"height"`
	} `json:"region"`
}

func TestSharedAtomicActionFixtureMatchesTheRustSummary(t *testing.T) {
	atomic := fixture[atomicAction](t, "conformance/actions/atomic-action.json")
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	region := atomic.Region
	recording := launch(
		t,
		client,
		atomic.Command,
		termtrace.LaunchOptions{Rows: new(region.Height), Cols: new(region.Width)},
	)
	_, err := recording.Screen().WaitForText(ctx, atomic.ReadyText, 5*time.Second)
	fatal(t, err)
	initial, err := recording.Screen().Frame(ctx, termtrace.Projection{})
	fatal(t, err)

	var predicate termtrace.ScreenPredicate
	fatal(t, termtrace.DecodeJSON(atomic.Predicate, &predicate))

	builder := recording.Actions().WithRequestID(atomic.RequestID).IfState(initial.StateHash)
	for _, atom := range atomic.Input {
		if atom.Kind == "text" {
			builder.SendText(atom.Text)
		} else {
			builder.SendKey(atom.Key)
		}
	}

	builder.UntilScreenWhere(predicate, 5*time.Second).Projection(termtrace.Projection{
		Detail: termtrace.FrameDetail(atomic.FrameDetail),
		Region: &termtrace.ScreenRegion{
			Top:    region.Top,
			Left:   region.Left,
			Right:  region.Left + region.Width,
			Bottom: region.Top + region.Height,
		},
	})
	result, err := builder.Run(ctx)
	fatal(t, err)
	retry, err := builder.Run(ctx)
	fatal(t, err)
	stable, err := recording.Screen().
		CaptureStable(ctx, termtrace.CaptureStableOptions{StableFor: 20 * time.Millisecond, Timeout: 2 * time.Second})
	fatal(t, err)

	frame := result.Frame()
	if frame == nil {
		t.Fatal("result has no frame")
	}

	quality := ""
	if screen, ok := result.Postcondition.(termtrace.ActionPostconditionResultScreen); ok {
		quality = string(screen.Quality)
	}

	first, err := json.Marshal(result.ActOutcome)
	fatal(t, err)
	second, err := json.Marshal(retry.ActOutcome)
	fatal(t, err)

	source, ok := frame.Source.(termtrace.ScreenFrameSourceActor)
	frameBoundaries, err := json.Marshal(source.Boundaries)
	fatal(t, err)
	resultBoundaries, err := json.Marshal(result.Boundaries)
	fatal(t, err)

	checks := map[string]bool{
		"state hash": len(frame.StateHash) == 64,
		"atom count": result.Input.AtomCount == atomic.ExpectedAtomCount,
		"matched": result.Matched() && (quality == "matched" || quality == "matched_unsettled") &&
			strings.Contains(frame.Text(), atomic.ExpectedText),
		"presented":        frame.Presented,
		"retry exact":      bytes.Equal(first, second),
		"stable":           stable.Quality == termtrace.StableCaptureQualityStable,
		"actor boundaries": ok && bytes.Equal(frameBoundaries, resultBoundaries),
	}
	for name, passed := range checks {
		if !passed {
			t.Errorf("%s does not match the Rust summary", name)
		}
	}
}

func TestLaunchResolvesReadyAndFailsPastItsReadinessDeadline(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf STARTED; sleep 30"},
		termtrace.LaunchOptions{},
	)
	status, err := recording.Status(ctx)
	fatal(t, err)

	if _, ok := status.(termtrace.SessionStatusLive); !ok {
		t.Errorf("status = %T", status)
	}

	_, err = recording.Screen().WaitForText(ctx, "STARTED", 5*time.Second)
	fatal(t, err)

	_, err = client.Launch(
		ctx,
		[]string{"/bin/sh", "-c", "sleep 30"},
		termtrace.LaunchOptions{StartupTimeoutMs: new(uint64(1))},
	)
	if err == nil || !strings.Contains(err.Error(), "readiness timed out") {
		t.Errorf("past readiness = %v", err)
	}
}

func TestLaunchRunsTheChildInTheRequestedWorkingDirectory(t *testing.T) {
	client, dir := newClient(t)
	ctx := within(t, 30*time.Second)
	workdir := filepath.Join(dir, "child-workdir")
	fatal(t, os.Mkdir(workdir, 0o755))
	fatal(t, os.WriteFile(filepath.Join(workdir, "marker-file.txt"), []byte("marker"), 0o644))
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", `printf "CWD=%s\r\n" "$(pwd -P)"; ls; printf DONE; sleep 30`},
		termtrace.LaunchOptions{Cwd: new(workdir), Cols: new(200)},
	)
	state, err := recording.Screen().WaitForText(ctx, "DONE", 5*time.Second)
	fatal(t, err)
	resolved, err := filepath.EvalSymlinks(workdir)
	fatal(t, err)

	if !strings.Contains(state.Text, "CWD="+resolved) ||
		!strings.Contains(state.Text, "marker-file.txt") {
		t.Errorf("screen = %q", state.Text)
	}

	if _, err := client.Launch(
		ctx,
		[]string{"/bin/sh", "-c", "sleep 30"},
		termtrace.LaunchOptions{Cwd: new(filepath.Join(dir, "missing"))},
	); err == nil {
		t.Error("launch in a missing directory succeeded")
	}
}

func waitForCompleted(
	t *testing.T,
	ctx context.Context,
	session *termtrace.Session,
) termtrace.SessionStatusCompleted {
	t.Helper()

	for {
		status, err := session.Status(ctx)
		if completed, ok := status.(termtrace.SessionStatusCompleted); err == nil && ok {
			return completed
		}

		select {
		case <-ctx.Done():
			t.Fatal("session did not complete")
		case <-time.After(50 * time.Millisecond):
		}
	}
}

func TestADetachedSessionKeepsRunningAndIsControlledThroughItsTarget(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(
		t,
		client,
		[]string{program(t, "line-command.sh")},
		termtrace.LaunchOptions{},
	)
	_, err := recording.Screen().WaitForText(ctx, "LINE_READY", 5*time.Second)
	fatal(t, err)
	fatal(t, recording.Detach(ctx))

	if active, err := recording.IsActive(ctx); err != nil || active {
		t.Errorf("active after detach = %v, %v", active, err)
	}

	if err := recording.Detach(ctx); err == nil {
		t.Error("second detach succeeded")
	}

	byPath, err := client.AttachTarget(ctx, "path:"+string(recording.Path()))
	fatal(t, err)

	if byPath.ID() != recording.ID() {
		t.Errorf("attached %s, want %s", byPath.ID(), recording.ID())
	}

	t.Cleanup(func() {
		if _, err := os.Stat(
			filepath.Join(string(recording.Path()), "completion.json"),
		); err != nil {
			_, _ = byPath.Signal(
				context.Background(),
				termtrace.ProcessSignalTerminate,
				termtrace.MutationOptions{},
			)
		}
	})

	status, err := byPath.Status(ctx)
	fatal(t, err)

	if _, ok := status.(termtrace.SessionStatusLive); !ok {
		t.Errorf("status = %T", status)
	}

	_, err = byPath.Actions().
		SendText("detached-input").
		SendKey("Enter").
		UntilScreen("LINE_VALUE=detached-input", 5*time.Second).
		Run(ctx)
	fatal(t, err)

	if completed := waitForCompleted(
		t,
		ctx,
		byPath,
	); completed.Completion.ExitCode == nil ||
		*completed.Completion.ExitCode != 0 {
		t.Errorf("completion = %+v", completed.Completion)
	}

	if _, err := client.AttachTarget(ctx, "name:unregistered-demo"); err == nil {
		t.Error("attached an unregistered name")
	}
}

func withReporter(t *testing.T, client *termtrace.Client) *termtrace.Recording {
	t.Helper()
	recording := launch(
		t,
		client,
		[]string{program(t, "input-reporter.py")},
		termtrace.LaunchOptions{Rows: new(40), Cols: new(80)},
	)
	_, err := recording.Screen().WaitForText(t.Context(), "REPORTER_READY", 5*time.Second)
	fatal(t, err)

	return recording
}

func TestPasteFocusAndScrollReachAnApplicationThatReportsThem(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := withReporter(t, client)

	pasted, err := recording.Actions().
		Paste("pasted words").
		UntilScreen("PASTE=", 5*time.Second).
		Run(ctx)
	fatal(t, err)

	text := pasted.Frame().Text()
	if len(reported(text, "PASTE=pasted words")) != 1 || len(reported(text, "KEY=70")) != 0 {
		t.Errorf("paste screen = %q", text)
	}

	pointer := recording.Pointer()
	for _, focused := range []bool{false, true} {
		outcome, err := pointer.Focus(ctx, focused, termtrace.FocusOptions{Strict: true})
		fatal(t, err)

		if _, ok := outcome.(termtrace.PointerOutcomeApplied); !ok {
			t.Errorf("focus %v = %+v", focused, outcome)
		}

		want := map[bool]string{false: "FOCUS=out", true: "FOCUS=in"}[focused]
		_, err = recording.Screen().WaitForText(ctx, want, 5*time.Second)
		fatal(t, err)
	}

	for _, scroll := range []struct{ column, row, lines int }{{4, 2, -2}, {9, 7, 1}} {
		outcome, err := pointer.Scroll(
			ctx,
			scroll.column,
			scroll.row,
			termtrace.ScrollOptions{Lines: scroll.lines, Strict: true},
		)
		fatal(t, err)

		if _, ok := outcome.(termtrace.PointerOutcomeApplied); !ok {
			t.Errorf("scroll = %+v", outcome)
		}
	}

	state, err := recording.Screen().WaitForText(ctx, "WHEEL=down", 5*time.Second)
	fatal(t, err)

	if len(reported(state.Text, "WHEEL=up@5,3")) != 2 ||
		len(reported(state.Text, "WHEEL=down@10,8")) != 1 {
		t.Errorf("wheel screen = %q", state.Text)
	}

	echo := withEcho(t, client, interactionFixture(t))
	focus, err := echo.Pointer().Focus(ctx, true, termtrace.FocusOptions{})
	fatal(t, err)
	scrolled, err := echo.Pointer().Scroll(ctx, 0, 0, termtrace.ScrollOptions{Lines: 1})
	fatal(t, err)

	_, focusNoop := focus.(termtrace.PointerOutcomeNoop)

	_, scrollNoop := scrolled.(termtrace.PointerOutcomeNoop)
	if !focusNoop || !scrollNoop {
		t.Errorf("without reporting: focus %+v, scroll %+v", focus, scrolled)
	}

	if _, err := echo.Pointer().Focus(ctx, true, termtrace.FocusOptions{Strict: true}); err == nil {
		t.Error("strict focus without reporting succeeded")
	}
}

func TestPacedTextArrivesOneCharacterAtATime(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := withReporter(t, client)
	_, err := recording.Actions().SendText("abc").UntilScreen("KEY=63", 5*time.Second).Run(ctx)
	fatal(t, err)
	result, err := recording.Actions().
		Pace(200*time.Millisecond).
		SendText("xyz").
		UntilScreen("KEY=7a", 5*time.Second).
		Run(ctx)
	fatal(t, err)

	text := result.Frame().Text()
	arrival := func(keys ...string) []int {
		times := make([]int, 0, len(keys))

		for _, key := range keys {
			found := reported(text, "KEY="+key)
			if len(found) == 0 {
				t.Fatalf("KEY=%s missing from %q", key, text)
			}

			times = append(times, found[0])
		}

		return times
	}
	burst := arrival("61", "62", "63")
	paced := arrival("78", "79", "7a")

	if burst[2]-burst[0] >= 100 {
		t.Errorf("burst spread %v", burst)
	}

	for index := 1; index < len(paced); index++ {
		if paced[index]-paced[index-1] < 150 {
			t.Errorf("paced arrivals %v", paced)
		}
	}
}

func TestWaitForIdleResolvesAfterOutputStops(t *testing.T) {
	client, _ := newClient(t)
	ticker := "i=0; while [ $i -lt 8 ]; do i=$((i+1)); printf 'TICK %d\\r\\n' $i; sleep 0.2; done"
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", ticker + "; printf SETTLED; sleep 30"},
		termtrace.LaunchOptions{},
	)
	started := time.Now()
	state, err := recording.Screen().WaitForIdle(t.Context(), 15*time.Second)
	fatal(t, err)

	if time.Since(started) < 1400*time.Millisecond || !strings.Contains(state.Text, "TICK 8") ||
		!strings.Contains(state.Text, "SETTLED") {
		t.Errorf("idle after %v: %q", time.Since(started), state.Text)
	}

	busy := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo BUSY; while :; do date +%s%N; sleep 0.1; done"},
		termtrace.LaunchOptions{},
	)
	_, err = busy.Screen().WaitForText(t.Context(), "BUSY", 5*time.Second)
	fatal(t, err)

	if _, err := busy.Screen().
		WaitForIdle(t.Context(), 1500*time.Millisecond); err == nil ||
		!strings.Contains(err.Error(), "stable interval") {
		t.Errorf("busy idle wait = %v", err)
	}
}

func TestDiscardCompletedRemovesOnlyACompletedSession(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)

	recording := withEcho(t, client, interactionFixture(t))
	if err := recording.DiscardCompleted(ctx); err == nil {
		t.Error("discarded a live session")
	}

	if _, err := os.Stat(string(recording.Path())); err != nil {
		t.Fatal(err)
	}

	fatal(t, recording.Stop(ctx))
	fatal(t, recording.DiscardCompleted(ctx))

	if _, err := os.Stat(string(recording.Path())); !os.IsNotExist(err) {
		t.Errorf("session directory remains: %v", err)
	}

	if _, err := client.Attach(ctx, string(recording.Path())); err == nil {
		t.Error("attached a discarded session")
	}

	if err := recording.DiscardCompleted(ctx); err == nil {
		t.Error("discarded twice")
	}
}

func TestSystemReportsCapabilitiesVersionsAndSchemas(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 60*time.Second)
	capabilities, err := client.Capabilities(ctx)
	fatal(t, err)

	if len(capabilities.Capabilities) == 0 {
		t.Error("no capabilities")
	}

	versions, err := client.Versions(ctx)
	fatal(t, err)

	if versions.Termtrace != termtrace.Version || versions.ControlProtocol == "" {
		t.Errorf("versions = %+v", versions)
	}

	names, err := client.Schemas(ctx)
	fatal(t, err)

	if len(names) == 0 {
		t.Fatal("no schemas")
	}

	schema, err := client.Schema(ctx, names[0])
	fatal(t, err)

	if schema.Kind() != '{' {
		t.Errorf("schema %s = %s", names[0], schema)
	}

	if _, err := client.Schema(ctx, "missing.schema.json"); err == nil {
		t.Error("returned a missing schema")
	}
}
