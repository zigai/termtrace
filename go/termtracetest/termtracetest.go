// Package termtracetest runs terminal tests with isolated Termtrace evidence. Recordings a test
// launches are stopped and verified when it ends, and their evidence is kept when it fails.
package termtracetest

import (
	"context"
	"errors"
	"fmt"
	"os"
	"slices"
	"sync"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

const finalizeBudget = time.Minute

var errFinalized = errors.New("termtracetest: terminal test is finalized")

// Options configures a terminal test.
//
//exhaustruct:optional
type Options struct {
	// Binary is the termtrace CLI; $TERMTRACE_BIN, then termtrace on PATH, by default.
	Binary string
	// EvidencePolicy decides when evidence is kept; on failure when empty.
	EvidencePolicy termtrace.EvidencePolicy
	// FinalizationTimeout bounds stopping and verifying each recording; 10 seconds when zero.
	FinalizationTimeout time.Duration
}

// Terminal is a test's Termtrace client with its own evidence root.
type Terminal struct {
	*termtrace.Client

	tb      testing.TB
	root    string
	options Options

	mutex      sync.Mutex
	recordings []*termtrace.Recording
	finalized  bool
	report     *termtrace.FinalizationReport //exhaustruct:optional
}

// New starts a client whose evidence goes to a new directory. When tb ends, every recording
// launched through [Terminal.Launch] is finalized: a passing test discards its evidence, and a
// failing one keeps it and logs its path. Cleanup failures fail the test.
func New(tb testing.TB, options Options) *Terminal {
	tb.Helper()

	//nolint:usetesting // reason: evidence must outlive a failing test; finalization removes it after a pass.
	root, err := os.MkdirTemp("", "termtrace-test-")
	if err != nil {
		tb.Fatalf("create evidence root: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), finalizeBudget)
	defer cancel()

	client, err := termtrace.New(
		ctx,
		termtrace.Options{Binary: options.Binary, EvidenceRoot: root},
	)
	if err != nil {
		tb.Fatalf("start termtrace: %v; evidence root %s", err, root)
	}

	terminal := &Terminal{Client: client, tb: tb, root: root, options: options}
	tb.Cleanup(terminal.cleanup)

	return terminal
}

// EvidenceRoot returns the directory that holds this test's sessions.
func (t *Terminal) EvidenceRoot() string {
	return t.root
}

// Launch starts command in a recorded terminal owned by the test.
func (t *Terminal) Launch(
	ctx context.Context,
	command []string,
	options termtrace.LaunchOptions,
) (*termtrace.Recording, error) {
	t.mutex.Lock()
	defer t.mutex.Unlock()

	if t.finalized {
		return nil, errFinalized
	}

	recording, err := t.Client.Launch(ctx, command, options)
	if err != nil {
		return nil, fmt.Errorf("launch %v: %w", command, err)
	}

	t.recordings = append(t.recordings, recording)

	return recording, nil
}

// Finalize ends the test's recordings now with outcome, and returns the report. Later calls
// and the cleanup at the end of the test return the same report. A failed call finalizes
// nothing, so the cleanup still does.
func (t *Terminal) Finalize(
	ctx context.Context,
	outcome termtrace.TestOutcome,
) (termtrace.FinalizationReport, error) {
	t.mutex.Lock()
	defer t.mutex.Unlock()

	if t.finalized {
		return *t.report, nil
	}

	report, err := t.FinalizeRecordings(
		ctx,
		slices.Clone(t.recordings),
		t.root,
		outcome,
		termtrace.FinalizeOptions{
			Policy:  t.options.EvidencePolicy,
			Timeout: t.options.FinalizationTimeout,
		},
	)
	if err != nil {
		return termtrace.FinalizationReport{}, fmt.Errorf("finalize terminal test: %w", err)
	}

	t.finalized = true
	t.report = &report

	return report, nil
}

// Report returns the finalization report, or nil before the test is finalized.
func (t *Terminal) Report() *termtrace.FinalizationReport {
	t.mutex.Lock()
	defer t.mutex.Unlock()

	return t.report
}

func (t *Terminal) cleanup() {
	outcome := termtrace.TestOutcomePassed
	if t.tb.Failed() || t.tb.Skipped() {
		outcome = termtrace.TestOutcomeError
	}

	ctx, cancel := context.WithTimeout(context.Background(), finalizeBudget)
	defer cancel()

	report, err := t.Finalize(ctx, outcome)
	closeErr := t.Close()

	failed := err != nil || closeErr != nil || len(report.SecondaryFailures) > 0
	if t.tb.Failed() || failed {
		t.tb.Logf("Termtrace test evidence: %s", t.evidence())
	}

	if err != nil {
		t.tb.Error(err)
	}

	if closeErr != nil {
		t.tb.Errorf("close termtrace: %v", closeErr)
	}

	for _, failure := range report.SecondaryFailures {
		t.tb.Errorf("termtrace cleanup %s: %s", failure.Phase, failure.Message)
	}
}

func (t *Terminal) evidence() string {
	if _, err := os.Stat(t.root); err != nil {
		return "no surviving evidence"
	}

	return t.root
}
