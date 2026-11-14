package consumer

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
	"github.com/zigai/termtrace/go/termtracetest"
)

var errOperational = errors.New("operational failure")

var echoLines = []string{
	"/bin/sh",
	"-c",
	`trap "exit 0" TERM; printf READY; while IFS= read -r line; do printf "INPUT:<%s>" "$line"; done`,
}

type assertionCorpus struct {
	Frame       string                          `json:"frame"`
	MaskOptions termtrace.MaskedSnapshotOptions `json:"mask_options"`
	Normalized  map[string]string               `json:"normalized"`
	Masked      string                          `json:"masked"`
	Mismatches  map[string]map[string]any       `json:"mismatches"`
}

// fakeTB stands in for a test that may fail, so harness cleanup can be observed without
// failing the real test.
type fakeTB struct {
	testing.TB

	mutex    sync.Mutex
	cleanups []func()
	failed   bool
	skipped  bool
	errors   []string
	logs     []string
}

func newFakeTB(t *testing.T) *fakeTB {
	t.Helper()

	fake := &fakeTB{TB: t, mutex: sync.Mutex{}, cleanups: nil, errors: nil, logs: nil}
	t.Cleanup(fake.finish)

	return fake
}

func (f *fakeTB) Cleanup(cleanup func()) {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	f.cleanups = append(f.cleanups, cleanup)
}

func (f *fakeTB) Fail() {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	f.failed = true
}

func (f *fakeTB) Failed() bool {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	return f.failed
}

func (f *fakeTB) Skipped() bool {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	return f.skipped
}

func (f *fakeTB) Error(args ...any) {
	f.record(fmt.Sprint(args...))
}

func (f *fakeTB) Errorf(format string, args ...any) {
	f.record(fmt.Sprintf(format, args...))
}

func (f *fakeTB) Log(args ...any) {
	f.log(fmt.Sprint(args...))
}

func (f *fakeTB) Logf(format string, args ...any) {
	f.log(fmt.Sprintf(format, args...))
}

func (f *fakeTB) Fatal(args ...any) {
	f.TB.Fatal(append([]any{"fake test: "}, args...)...)
}

func (f *fakeTB) Fatalf(format string, args ...any) {
	f.TB.Fatalf("fake test: "+format, args...)
}

func (f *fakeTB) record(message string) {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	f.failed = true
	f.errors = append(f.errors, message)
}

func (f *fakeTB) log(message string) {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	f.logs = append(f.logs, message)
}

func (f *fakeTB) finish() {
	for {
		f.mutex.Lock()
		if len(f.cleanups) == 0 {
			f.mutex.Unlock()

			return
		}

		last := f.cleanups[len(f.cleanups)-1]
		f.cleanups = f.cleanups[:len(f.cleanups)-1]
		f.mutex.Unlock()

		last()
	}
}

func (f *fakeTB) errorMessages() []string {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	return slices.Clone(f.errors)
}

func (f *fakeTB) logMessages() []string {
	f.mutex.Lock()
	defer f.mutex.Unlock()

	return slices.Clone(f.logs)
}

// harness starts a terminal test on a fake test. Its evidence root is removed after the real
// test ends.
func harness(t *testing.T, options termtracetest.Options) (*fakeTB, *termtracetest.Terminal) {
	t.Helper()

	var root string

	t.Cleanup(func() { removeEvidence(t, root) })

	fake := newFakeTB(t)
	options.Binary = binary(t)
	terminal := termtracetest.New(fake, options)
	root = terminal.EvidenceRoot()

	return fake, terminal
}

func removeEvidence(t *testing.T, root string) {
	t.Helper()

	if root == "" {
		return
	}

	if err := os.RemoveAll(root); err != nil {
		t.Errorf("remove evidence root: %v", err)
	}
}

func launchReady(
	t *testing.T,
	ctx context.Context,
	terminal *termtracetest.Terminal,
	command []string,
	options termtrace.LaunchOptions,
) *termtrace.Recording {
	t.Helper()

	recording, err := terminal.Launch(ctx, command, options)
	fatal(t, err)
	_, err = recording.Screen().WaitForText(ctx, "READY", 5*time.Second)
	fatal(t, err)

	return recording
}

func requireFile(t *testing.T, path string) {
	t.Helper()

	if _, err := os.Stat(path); err != nil {
		t.Errorf("want %s: %v", path, err)
	}
}

func pathExists(path string) bool {
	_, err := os.Lstat(path)

	return err == nil
}

func verifiedContent(t *testing.T, path termtrace.Path) {
	t.Helper()

	client, _ := newClient(t)
	report, err := client.Integrity(string(path)).
		Verify(t.Context(), termtrace.VerifyOptions{Level: termtrace.VerificationLevelContent})
	fatal(t, err)

	if !report.Valid {
		t.Errorf("content verification of %s failed: %+v", path, report)
	}
}

func hasPhase(report *termtrace.FinalizationReport, phase string) bool {
	return slices.ContainsFunc(
		report.SecondaryFailures,
		func(failure termtrace.SecondaryFailure) bool {
			return failure.Phase == phase
		},
	)
}

func evidenceLogged(fake *fakeTB, root string) bool {
	return slices.Contains(fake.logMessages(), "Termtrace test evidence: "+root)
}

func requireReport(t *testing.T, terminal *termtracetest.Terminal) *termtrace.FinalizationReport {
	t.Helper()

	report := terminal.Report()
	if report == nil {
		t.Fatal("terminal test was not finalized")
	}

	return report
}

func corpusFrame(t *testing.T) (assertionCorpus, termtrace.ScreenFrame) {
	t.Helper()

	corpus := fixture[assertionCorpus](t, "conformance/testing/assertions.json")
	data, err := os.ReadFile(
		filepath.Join(repo(t), "fixtures", "conformance", "testing", corpus.Frame),
	)
	fatal(t, err)

	var frame termtrace.ScreenFrame
	fatal(t, termtrace.DecodeJSON(data, &frame))

	return corpus, frame
}

func mismatchFields(t *testing.T, err error) map[string]any {
	t.Helper()

	failure := termtraceError(t, err)
	if !termtrace.IsExpectationMismatch(err) || failure.Facts.Mismatch == nil {
		t.Fatalf("want an expectation mismatch, got %v", err)
	}

	data, marshalErr := json.Marshal(failure.Facts.Mismatch)
	fatal(t, marshalErr)

	fields := map[string]any{}
	fatal(t, json.Unmarshal(data, &fields))

	return fields
}

func requireOperational(t *testing.T, name string, err error) {
	t.Helper()

	if err == nil || termtrace.IsExpectationMismatch(err) {
		t.Errorf("%s: want an operational failure, got %v", name, err)
	}
}

func TestAssertionFamiliesMatchTheSharedCorpus(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	corpus, frame := corpusFrame(t)
	found, err := client.Query(ctx, frame, termtrace.Text("Save"))
	fatal(t, err)
	none, err := client.Query(ctx, frame, termtrace.Text("ABSENT"))
	fatal(t, err)
	selected, err := found.First(ctx)
	fatal(t, err)

	for profile, expected := range corpus.Normalized {
		snapshot, err := client.NormalizedSnapshot(ctx, frame, termtrace.SnapshotOptions{
			Profile:       termtrace.SnapshotProfile(profile),
			IncludeCursor: true,
			IncludeModes:  true,
		})
		fatal(t, err)

		if snapshot != expected {
			t.Errorf("%s snapshot = %q, want %q", profile, snapshot, expected)
		}
	}

	masked, err := client.MaskedSnapshot(ctx, frame, corpus.MaskOptions)
	fatal(t, err)

	if masked != corpus.Masked {
		t.Errorf("masked snapshot = %q, want %q", masked, corpus.Masked)
	}

	for name, err := range map[string]error{
		"screen_text":            client.AssertScreenText(ctx, frame, "Save"),
		"exists":                 found.AssertExists(ctx),
		"not_exists":             none.AssertNotExists(ctx),
		"count":                  found.AssertCount(ctx, 2),
		"match_text":             selected.AssertText(ctx, "Save"),
		"match_style":            selected.AssertStyle(ctx, termtrace.StyleFilter{}),
		"cursor":                 client.AssertCursor(ctx, frame, frame.Cursor),
		"no_presented_transient": client.AssertNoPresentedTransient(ctx, frame),
	} {
		if err != nil {
			t.Errorf("%s: %v", name, err)
		}
	}

	moved := frame.Cursor
	moved.Row = new(0)
	transient := frame
	transient.Presented = false
	failing := map[string]func() error{
		"screen_text": func() error { return client.AssertScreenText(ctx, frame, "ABSENT") },
		"exists":      func() error { return none.AssertExists(ctx) },
		"not_exists":  func() error { return found.AssertNotExists(ctx) },
		"count":       func() error { return found.AssertCount(ctx, 1) },
		"match_text":  func() error { return selected.AssertText(ctx, "OTHER") },
		"match_style": func() error {
			return selected.AssertStyle(ctx, termtrace.StyleFilter{Bold: new(true)})
		},
		"cursor":          func() error { return client.AssertCursor(ctx, frame, moved) },
		"no_wrapped_rows": func() error { return client.AssertNoWrappedRows(ctx, frame) },
		"neutral_region": func() error {
			region := termtrace.ScreenRegion{Top: 0, Left: 0, Right: 1, Bottom: 1}

			return client.AssertNeutralRegion(ctx, frame, region)
		},
		"no_presented_transient": func() error {
			return client.AssertNoPresentedTransient(ctx, transient)
		},
	}

	for name, call := range failing {
		if len(corpus.Mismatches[name]) == 0 {
			t.Errorf("corpus has no %s mismatch", name)
		}

		fields := mismatchFields(t, call())
		for key, expected := range corpus.Mismatches[name] {
			if !reflect.DeepEqual(fields[key], expected) {
				t.Errorf("%s %s = %#v, want %#v", name, key, fields[key], expected)
			}
		}

		if err := client.AssertNot(ctx, call); err != nil {
			t.Errorf("negated %s: %v", name, err)
		}
	}

	operations := map[string]func() error{
		"invalid regex": func() error {
			_, err := client.Query(ctx, frame, termtrace.Regex("["))

			return fmt.Errorf("query: %w", err)
		},
		"single of two": func() error {
			_, err := found.Single(ctx)

			return fmt.Errorf("single: %w", err)
		},
		"first of none": func() error {
			_, err := none.First(ctx)

			return fmt.Errorf("first: %w", err)
		},
	}
	for name, operation := range operations {
		requireOperational(t, name, client.AssertNot(ctx, operation))
	}
}

func TestSnapshotFilesUpdateOnlyOnRequest(t *testing.T) {
	client, dir := newClient(t)
	ctx := within(t, 30*time.Second)
	corpus, frame := corpusFrame(t)
	baseline := filepath.Join(dir, "baseline.json")

	err := client.AssertSnapshotFile(ctx, frame, baseline, termtrace.SnapshotFileOptions{})
	if !termtrace.IsExpectationMismatch(err) {
		t.Fatalf("missing baseline = %v, want a mismatch", err)
	}

	update := termtrace.SnapshotFileOptions{Update: true}
	fatal(t, client.AssertSnapshotFile(ctx, frame, baseline, update))
	fatal(t, client.AssertSnapshotFile(ctx, frame, baseline, termtrace.SnapshotFileOptions{}))

	original, err := os.ReadFile(baseline)
	fatal(t, err)
	normalized, err := client.NormalizedSnapshot(ctx, frame, termtrace.SnapshotOptions{})
	fatal(t, err)

	if string(original) != normalized+"\n" {
		t.Errorf("baseline = %q, want %q", original, normalized+"\n")
	}

	link := filepath.Join(dir, "link")
	fatal(t, os.Symlink(baseline, link))

	for _, update := range []bool{false, true} {
		err := client.AssertNot(ctx, func() error {
			return client.AssertSnapshotFile(
				ctx,
				frame,
				link,
				termtrace.SnapshotFileOptions{Update: update},
			)
		})
		requireOperational(t, "symlink baseline update="+strconv.FormatBool(update), err)
	}

	invalid := termtrace.MaskedSnapshotOptions{
		MaskRegions: []termtrace.ScreenRegion{{Top: 1, Left: 0, Right: 1, Bottom: 2}},
	}
	err = client.AssertMaskedSnapshotFile(ctx, frame, baseline, invalid, true)

	if failure := termtraceError(t, err); failure.Kind() != "invalid_query" {
		t.Errorf("invalid mask kind = %q, want invalid_query: %v", failure.Kind(), err)
	}

	after, err := os.ReadFile(baseline)
	fatal(t, err)

	if string(after) != string(original) {
		t.Errorf("rejected update changed the baseline to %q", after)
	}

	masked := filepath.Join(dir, "masked.json")
	fatal(t, client.AssertMaskedSnapshotFile(ctx, frame, masked, corpus.MaskOptions, true))
	fatal(t, client.AssertMaskedSnapshotFile(ctx, frame, masked, corpus.MaskOptions, false))

	bare := frame
	bare.Data = termtrace.ScreenFrameDataNone{}
	err = client.AssertNot(ctx, func() error { return client.AssertScreenText(ctx, bare, "Save") })

	if failure := termtraceError(t, err); failure.Kind() != "insufficient_frame_detail" {
		t.Errorf("frame without rows kind = %q: %v", failure.Kind(), err)
	}
}

func TestAssertNotPassesOnlyOnMismatch(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	_, frame := corpusFrame(t)

	err := client.AssertNot(
		ctx,
		func() error { return client.AssertScreenText(ctx, frame, "ABSENT") },
	)
	if err != nil {
		t.Errorf("negated mismatch: %v", err)
	}

	err = client.AssertNot(ctx, func() error { return errOperational })
	if !errors.Is(err, errOperational) || err.Error() != errOperational.Error() {
		t.Errorf("operational failure = %v, want it unchanged", err)
	}

	err = client.AssertNot(ctx, func() error { return client.AssertScreenText(ctx, frame, "Save") })
	fields := mismatchFields(t, err)

	if fields["assertion"] != "not" {
		t.Errorf("held expectation assertion = %v, want not", fields["assertion"])
	}

	if frame, ok := fields["frame"]; ok && frame != nil {
		t.Errorf("held expectation frame = %v, want none", frame)
	}
}

func TestTerminalTestRetentionFollowsPolicy(t *testing.T) {
	for _, policy := range []termtrace.EvidencePolicy{
		termtrace.EvidencePolicyOnFailure,
		termtrace.EvidencePolicyAlways,
		termtrace.EvidencePolicyManual,
	} {
		t.Run(string(policy), func(t *testing.T) {
			fake, terminal := harness(t, termtracetest.Options{EvidencePolicy: policy})
			ctx := within(t, 30*time.Second)
			recording := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
			_, err := recording.Actions().
				SendText("go\n").
				UntilScreen("INPUT:<go>", 5*time.Second).
				Run(ctx)
			fatal(t, err)
			frame, err := recording.Screen().Frame(ctx, termtrace.Projection{})
			fatal(t, err)
			fatal(t, terminal.AssertScreenText(ctx, frame, "INPUT:<go>"))

			report, err := terminal.Finalize(ctx, termtrace.TestOutcomePassed)
			fatal(t, err)
			again, err := terminal.Finalize(ctx, termtrace.TestOutcomeError)
			fatal(t, err)

			if !reflect.DeepEqual(again, report) || !reflect.DeepEqual(*terminal.Report(), report) {
				t.Errorf(
					"later reports %+v and %+v differ from %+v",
					again,
					*terminal.Report(),
					report,
				)
			}

			if _, err := terminal.Launch(ctx, echoLines, termtrace.LaunchOptions{}); err == nil {
				t.Error("launch after finalization succeeded")
			}

			if active, err := recording.IsActive(ctx); err != nil || active {
				t.Errorf("active after finalization = %v, %v", active, err)
			}

			fake.finish()

			if len(report.SecondaryFailures) != 0 {
				t.Errorf("secondary failures = %+v", report.SecondaryFailures)
			}

			if report.Retained != (policy != termtrace.EvidencePolicyOnFailure) {
				t.Errorf("retained = %v", report.Retained)
			}

			if pathExists(terminal.EvidenceRoot()) != report.Retained {
				t.Errorf("evidence root exists = %v, retained = %v",
					pathExists(terminal.EvidenceRoot()), report.Retained)
			}

			if messages := fake.errorMessages(); len(messages) != 0 {
				t.Errorf("passing test failed at cleanup: %q", messages)
			}

			if report.Retained {
				verifiedContent(t, recording.Path())
			}
		})
	}
}

func TestTerminalTestLifecycle(t *testing.T) {
	var root string

	t.Cleanup(func() { removeEvidence(t, root) })

	terminal := termtracetest.New(t, termtracetest.Options{Binary: binary(t)})
	root = terminal.EvidenceRoot()
	ctx := within(t, 30*time.Second)
	recording, err := terminal.Launch(
		ctx,
		[]string{"/bin/sh", "-c", "echo HELPER_TEST_OK"},
		termtrace.LaunchOptions{},
	)
	fatal(t, err)
	state, err := recording.Screen().WaitForText(ctx, "HELPER_TEST_OK", 5*time.Second)
	fatal(t, err)

	if !strings.Contains(state.Text, "HELPER_TEST_OK") {
		t.Errorf("screen text = %q", state.Text)
	}
}

func TestFailingTerminalTestKeepsVerifiedEvidence(t *testing.T) {
	for name, skipped := range map[string]bool{"failed": false, "skipped": true} {
		t.Run(name, func(t *testing.T) {
			fake, terminal := harness(t, termtracetest.Options{})
			ctx := within(t, 30*time.Second)
			recording := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
			frame, err := recording.Screen().Frame(ctx, termtrace.Projection{})
			fatal(t, err)
			fatal(t, terminal.AssertScreenText(ctx, frame, "READY"))

			if skipped {
				fake.skipped = true
			} else {
				fake.Errorf("primary failure")
			}

			fake.finish()

			report := requireReport(t, terminal)
			if !report.Retained || report.Outcome != termtrace.TestOutcomeError ||
				len(report.SecondaryFailures) != 0 {
				t.Errorf("report = %+v", *report)
			}

			for _, artifact := range report.VerifiedArtifacts {
				requireFile(t, artifact)
			}

			requireFile(t, filepath.Join(string(recording.Path()), "events.jsonl"))
			requireFile(t, filepath.Join(string(recording.Path()), "completion.json"))
			verifiedContent(t, recording.Path())

			if !skipped && !evidenceLogged(fake, terminal.EvidenceRoot()) {
				t.Errorf("logs %q do not name the evidence root", fake.logMessages())
			}

			wantErrors := 1
			if skipped {
				wantErrors = 0
			}

			if messages := fake.errorMessages(); len(messages) != wantErrors {
				t.Errorf("errors = %q", messages)
			}
		})
	}
}

func TestCleanupFailureFailsAPassingTestAndFinalizesSiblings(t *testing.T) {
	fake, terminal := harness(
		t,
		termtracetest.Options{FinalizationTimeout: 100 * time.Millisecond},
	)
	ctx := within(t, 30*time.Second)
	first := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
	second := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
	fatal(t, first.ForceClose(ctx))

	fake.finish()

	report := requireReport(t, terminal)
	if len(report.SecondaryFailures) == 0 || !report.Retained {
		t.Errorf("report = %+v", *report)
	}

	for _, artifact := range report.VerifiedArtifacts {
		requireFile(t, artifact)
	}

	requireFile(t, filepath.Join(string(second.Path()), "completion.json"))

	if !fake.Failed() || len(fake.errorMessages()) == 0 {
		t.Error("cleanup failure did not fail the test")
	}

	if !evidenceLogged(fake, terminal.EvidenceRoot()) {
		t.Errorf("logs %q do not name the evidence root", fake.logMessages())
	}
}

func TestInvalidFinalizeOutcomeStillFinalizesAtCleanup(t *testing.T) {
	fake, terminal := harness(t, termtracetest.Options{})
	ctx := within(t, 30*time.Second)
	recording := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})

	_, err := terminal.Finalize(ctx, termtrace.TestOutcome("invalid"))
	if err == nil {
		t.Fatal("invalid outcome was accepted")
	}

	fake.Errorf("primary failure: %v", err)
	fake.finish()

	report := requireReport(t, terminal)
	if !report.Retained || report.Outcome != termtrace.TestOutcomeError {
		t.Errorf("report = %+v", *report)
	}

	requireFile(t, filepath.Join(string(recording.Path()), "completion.json"))
}

func TestFinalizeRecordingsPreparesOneArtifactDirectory(t *testing.T) {
	client, dir := newClient(t)
	ctx := within(t, 30*time.Second)
	root := filepath.Join(dir, "sessions")
	recording := launch(t, client, echoLines, termtrace.LaunchOptions{})
	_, err := recording.Screen().WaitForText(ctx, "READY", 5*time.Second)
	fatal(t, err)

	report, err := client.FinalizeRecordings(
		ctx,
		[]*termtrace.Recording{recording},
		root,
		termtrace.TestOutcomePassed,
		termtrace.FinalizeOptions{Policy: termtrace.EvidencePolicyManual, Timeout: time.Second},
	)
	fatal(t, err)

	if !report.Retained || len(report.SecondaryFailures) != 0 {
		t.Errorf("report = %+v", report)
	}

	entries, err := os.ReadDir(root)
	fatal(t, err)

	artifacts := 0

	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".test-artifacts-") {
			artifacts++
		}
	}

	if artifacts != 1 {
		t.Errorf("artifact directories = %d, want 1", artifacts)
	}

	requireFile(t, filepath.Join(string(recording.Path()), "completion.json"))
}

func TestFailedLaunchKeepsEarlierSessionsFinalized(t *testing.T) {
	fake, terminal := harness(t, termtracetest.Options{})
	ctx := within(t, 30*time.Second)
	recording := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})

	_, err := terminal.Launch(
		ctx,
		echoLines,
		termtrace.LaunchOptions{Cwd: new(filepath.Join(t.TempDir(), "missing"))},
	)
	if err == nil {
		t.Fatal("launch in a missing directory succeeded")
	}

	fake.Errorf("primary failure: %v", err)
	fake.finish()

	if report := requireReport(t, terminal); !report.Retained {
		t.Errorf("report = %+v", *report)
	}

	requireFile(t, filepath.Join(string(recording.Path()), "completion.json"))

	if messages := fake.errorMessages(); len(messages) != 1 {
		t.Errorf("errors = %q, want only the primary failure", messages)
	}
}

func TestPartialDiscardReportsOnlySurvivingPaths(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Fatal("permission failure needs an unprivileged user")
	}

	fake, terminal := harness(t, termtracetest.Options{})
	ctx := within(t, 30*time.Second)
	recording := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
	locked := filepath.Join(terminal.EvidenceRoot(), "locked")
	fatal(t, os.Mkdir(locked, 0o700))
	fatal(t, os.WriteFile(filepath.Join(locked, "entry"), []byte("survives"), 0o600))
	fatal(t, os.Chmod(locked, 0))
	t.Cleanup(func() {
		if pathExists(locked) {
			_ = os.Chmod(locked, 0o700)
		}
	})

	fake.finish()

	report := requireReport(t, terminal)
	if pathExists(string(recording.Path())) {
		t.Error("canonical session survived a discard")
	}

	if report.Retained != pathExists(terminal.EvidenceRoot()) {
		t.Errorf("retained = %v, root exists = %v", report.Retained, !report.Retained)
	}

	if report.EvidenceDirectory == nil || *report.EvidenceDirectory != terminal.EvidenceRoot() {
		t.Errorf(
			"evidence directory = %v, want %s",
			report.EvidenceDirectory,
			terminal.EvidenceRoot(),
		)
	}

	for _, artifact := range report.VerifiedArtifacts {
		requireFile(t, artifact)
	}

	if !hasPhase(report, "evidence_discard") {
		t.Errorf("secondary failures = %+v", report.SecondaryFailures)
	}

	if !fake.Failed() {
		t.Error("discard failure did not fail the test")
	}
}

func TestArtifactDirectoryFailureStillFinalizesEverySession(t *testing.T) {
	fake, terminal := harness(t, termtracetest.Options{})
	ctx := within(t, 30*time.Second)
	first := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
	second := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
	collision := filepath.Join(terminal.EvidenceRoot(), ".test-artifacts-"+first.ID())
	fatal(t, os.WriteFile(collision, []byte("collision"), 0o600))

	fake.finish()

	if report := requireReport(t, terminal); !hasPhase(report, "artifact_directory") {
		t.Errorf("secondary failures = %+v", report.SecondaryFailures)
	}

	for _, recording := range []*termtrace.Recording{first, second} {
		requireFile(t, filepath.Join(string(recording.Path()), "completion.json"))
	}

	if !fake.Failed() {
		t.Error("artifact directory failure did not fail the test")
	}
}

func TestDirectClientRecordingsStayCallerOwned(t *testing.T) {
	fake, terminal := harness(t, termtracetest.Options{})
	ctx := within(t, 30*time.Second)
	owned := launchReady(t, ctx, terminal, echoLines, termtrace.LaunchOptions{})
	foreign, err := terminal.Client.Launch(ctx, echoLines, termtrace.LaunchOptions{})
	fatal(t, err)
	_, err = foreign.Screen().WaitForText(ctx, "READY", 5*time.Second)
	fatal(t, err)

	report, err := terminal.Finalize(ctx, termtrace.TestOutcomePassed)
	fatal(t, err)

	if active, err := owned.IsActive(ctx); err != nil || active {
		t.Errorf("owned active = %v, %v", active, err)
	}

	if active, err := foreign.IsActive(ctx); err != nil || !active {
		t.Errorf("foreign active = %v, %v", active, err)
	}

	requireFile(t, filepath.Join(string(foreign.Path()), "session.json"))

	if !report.Retained || slices.Contains(report.VerifiedArtifacts, string(foreign.Path())) {
		t.Errorf("report = %+v", report)
	}

	fatal(t, foreign.Stop(ctx))
	fake.finish()
}

func TestStalledLifecycleCallCannotBlockForceClose(t *testing.T) {
	for _, operation := range []string{"stop", "completion"} {
		t.Run(operation, func(t *testing.T) {
			pids := t.TempDir()
			pidCommand := func(name string) []string {
				return []string{
					"/bin/sh",
					"-c",
					`printf "%s" "$PPID" > "$1"; printf "%s" $$ > "$1.child"; printf READY; ` +
						`while IFS= read -r line; do :; done`,
					"owned-lifecycle",
					filepath.Join(pids, name),
				}
			}
			small := termtrace.LaunchOptions{Rows: new(3), Cols: new(20)}
			fake, terminal := harness(
				t,
				termtracetest.Options{FinalizationTimeout: 250 * time.Millisecond},
			)
			ctx := within(t, 30*time.Second)
			started := time.Now()
			actor := launchReady(t, ctx, terminal, pidCommand("actor"), small)
			sibling := launchReady(t, ctx, terminal, pidCommand("sibling"), small)
			recorder := pidOfFile(t, filepath.Join(pids, "actor"))

			cmdline, err := os.ReadFile(fmt.Sprintf("/proc/%d/cmdline", recorder))
			fatal(t, err)

			if !strings.Contains(string(cmdline), string(actor.Path())) {
				t.Fatalf("pid %d is not the recorder of %s", recorder, actor.Path())
			}

			fatal(t, syscall.Kill(recorder, syscall.SIGSTOP))

			worker := make(chan struct{})

			go func() {
				defer close(worker)

				if operation == "stop" {
					_, _ = actor.RequestStop(ctx, termtrace.StopOptions{Wait: 30 * time.Second})
				} else {
					_, _ = actor.WaitForCompletion(ctx, 30*time.Second)
				}
			}()

			for range 1000 {
				active, err := actor.IsActive(ctx)
				fatal(t, err)

				if !active {
					break
				}

				time.Sleep(time.Millisecond)
			}

			if active, _ := actor.IsActive(ctx); active {
				t.Fatal("lifecycle call did not take the recording")
			}

			fake.Errorf("primary failure")
			fake.finish()

			if elapsed := time.Since(started); elapsed > 8*time.Second {
				t.Errorf("finalization took %v: %+v", elapsed, terminal.Report())
			}

			<-worker

			if report := requireReport(t, terminal); !report.Retained {
				t.Errorf("report = %+v", *report)
			}

			requireFile(t, filepath.Join(string(sibling.Path()), "completion.json"))

			for _, name := range []string{"actor", "actor.child", "sibling", "sibling.child"} {
				pid := pidOfFile(t, filepath.Join(pids, name))
				if pathExists(fmt.Sprintf("/proc/%d", pid)) {
					t.Errorf("%s process %d was not reaped", name, pid)
				}
			}
		})
	}
}

func pidOfFile(t *testing.T, path string) int {
	t.Helper()

	data, err := os.ReadFile(path)
	fatal(t, err)
	pid, err := strconv.Atoi(string(data))
	fatal(t, err)

	return pid
}

func TestRecorderCompletionIsClassifiedTruthfully(t *testing.T) {
	small := termtrace.LaunchOptions{Rows: new(3), Cols: new(20)}

	t.Run("spawn failed", func(t *testing.T) {
		fake, terminal := harness(t, termtracetest.Options{})
		ctx := within(t, 30*time.Second)
		recording, err := terminal.Launch(ctx, []string{"/termtrace-no-such-child"}, small)
		fatal(t, err)
		completion, err := recording.WaitForCompletion(ctx, 5*time.Second)
		fatal(t, err)

		if completion.Failure == nil || completion.Failure.Code != "spawn_failed" {
			t.Errorf("completion failure = %+v", completion.Failure)
		}

		fake.finish()

		report := requireReport(t, terminal)
		classified := slices.ContainsFunc(
			report.SecondaryFailures,
			func(failure termtrace.SecondaryFailure) bool {
				return failure.Phase == "session_failure" &&
					strings.Contains(failure.Message, "spawn_failed")
			},
		)

		if !classified || !report.Retained {
			t.Errorf("report = %+v", *report)
		}

		if !fake.Failed() {
			t.Error("a recorder failure did not fail the test")
		}
	})

	t.Run("exit 7", func(t *testing.T) {
		fake, terminal := harness(t, termtracetest.Options{})
		ctx := within(t, 30*time.Second)
		recording, err := terminal.Launch(ctx, []string{"/bin/sh", "-c", "exit 7"}, small)
		fatal(t, err)
		completion, err := recording.WaitForCompletion(ctx, 5*time.Second)
		fatal(t, err)

		if completion.ExitCode == nil || *completion.ExitCode != 7 || completion.Failure != nil {
			t.Errorf("completion = %+v", completion)
		}

		fake.finish()

		report := requireReport(t, terminal)
		if report.Retained || len(report.SecondaryFailures) != 0 {
			t.Errorf("report = %+v", *report)
		}

		if messages := fake.errorMessages(); len(messages) != 0 {
			t.Errorf("errors = %q", messages)
		}
	})
}
