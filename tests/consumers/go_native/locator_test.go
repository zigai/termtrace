package consumer

import (
	"context"
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

type parityCase struct {
	Name      string `json:"name"`
	Operation string `json:"operation"`
	Index     int    `json:"index"`
	Spec      struct {
		Text           *string                 `json:"text"`
		Exact          *string                 `json:"exact"`
		Regex          *string                 `json:"regex"`
		Row            *int                    `json:"row"`
		Region         *termtrace.ScreenRegion `json:"region"`
		Style          *termtrace.StyleFilter  `json:"style"`
		CrossSoftWraps bool                    `json:"cross_soft_wraps"`
	} `json:"spec"`
	Expect struct {
		Matches []parityMatch  `json:"matches"`
		Error   map[string]any `json:"error"`
	} `json:"expect"`
}

type parityMatch struct {
	Text   string  `json:"text"`
	Ranges [][]int `json:"ranges"`
	Widths []int   `json:"widths"`
}

type parityFixture struct {
	Frame      string         `json:"frame"`
	FrameFacts map[string]any `json:"frame_facts"`
	Cases      []parityCase   `json:"cases"`
	Live       struct {
		Command []string     `json:"command"`
		Rows    int          `json:"rows"`
		Cols    int          `json:"cols"`
		Ready   string       `json:"ready"`
		Cases   []parityCase `json:"cases"`
	} `json:"live"`
}

func paritySpec(testCase parityCase) termtrace.LocatorSpec {
	spec := termtrace.LocatorSpec{}

	switch {
	case testCase.Spec.Text != nil:
		spec = termtrace.Text(*testCase.Spec.Text)
	case testCase.Spec.Exact != nil:
		spec = termtrace.ExactText(*testCase.Spec.Exact)
	case testCase.Spec.Regex != nil:
		spec = termtrace.Regex(*testCase.Spec.Regex)
	}

	if testCase.Spec.Row != nil {
		spec = spec.InRow(*testCase.Spec.Row)
	}

	if testCase.Spec.Region != nil {
		spec = spec.InRegion(*testCase.Spec.Region)
	}

	if testCase.Spec.Style != nil {
		spec = spec.WithStyle(*testCase.Spec.Style)
	}

	if testCase.Spec.CrossSoftWraps {
		spec = spec.AcrossSoftWraps(true)
	}

	return spec
}

func triples(ranges []termtrace.MatchRange) [][]int {
	out := make([][]int, len(ranges))
	for index, span := range ranges {
		out[index] = []int{span.Row, span.StartColumn, span.EndColumn}
	}

	return out
}

func runParityQuery(
	t *testing.T,
	ctx context.Context,
	client *termtrace.Client,
	frame termtrace.ScreenFrame,
	testCase parityCase,
) ([]termtrace.ScreenMatch, error) {
	t.Helper()

	result, err := client.Query(ctx, frame, paritySpec(testCase))
	if err != nil {
		return nil, fmt.Errorf("query: %w", err)
	}

	var match termtrace.ScreenMatch

	switch testCase.Operation {
	case "":
		return result.Matches, nil
	case "single":
		match, err = result.Single(ctx)
	case "first":
		match, err = result.First(ctx)
	case "nth":
		match, err = result.Nth(ctx, testCase.Index)
	default:
		t.Fatalf("%s: unknown operation %s", testCase.Name, testCase.Operation)
	}

	if err != nil {
		return nil, fmt.Errorf("%s: %w", testCase.Operation, err)
	}

	return []termtrace.ScreenMatch{match}, nil
}

func checkParityFailure(t *testing.T, name string, err error, expect map[string]any) {
	t.Helper()

	failure := termtraceError(t, err)
	if failure.Kind() != expect["kind"] {
		t.Errorf("%s: kind %s, want %v", name, failure.Kind(), expect["kind"])
	}

	data, err := json.Marshal(failure.Facts.Locator)
	fatal(t, err)

	var details struct {
		Count     *int                   `json:"count"`
		Index     *int                   `json:"index"`
		StateHash *string                `json:"state_hash"`
		Ranges    []termtrace.MatchRange `json:"ranges"`
	}
	fatal(t, json.Unmarshal(data, &details))

	if want, ok := expect["count"].(float64); ok && expect["kind"] != "no_match" &&
		(details.Count == nil || *details.Count != int(want)) {
		t.Errorf("%s: count %v, want %v", name, details.Count, want)
	}

	if want, ok := expect["index"].(float64); ok &&
		(details.Index == nil || *details.Index != int(want)) {
		t.Errorf("%s: index %v, want %v", name, details.Index, want)
	}

	if want, ok := expect["state_hash"].(string); ok &&
		(details.StateHash == nil || *details.StateHash != want) {
		t.Errorf("%s: state hash %v, want %v", name, details.StateHash, want)
	}

	if want, ok := expect["ranges"]; ok && !sameJSON(t, triples(details.Ranges), want) {
		t.Errorf("%s: ranges %v, want %v", name, triples(details.Ranges), want)
	}
}

func checkParityCase(
	t *testing.T,
	ctx context.Context,
	client *termtrace.Client,
	frame termtrace.ScreenFrame,
	testCase parityCase,
) {
	t.Helper()

	name := testCase.Name

	found, err := runParityQuery(t, ctx, client, frame, testCase)
	if err != nil {
		if testCase.Expect.Error == nil {
			t.Errorf("%s: %v", name, err)
		} else {
			checkParityFailure(t, name, err, testCase.Expect.Error)
		}

		return
	}

	if testCase.Expect.Error != nil {
		t.Errorf("%s: unexpected success", name)
		return
	}

	observed := make([]parityMatch, len(found))
	for index, match := range found {
		widths := make([]int, len(match.Cells))
		for cell, covered := range match.Cells {
			widths[cell] = covered.Width
		}

		observed[index] = parityMatch{
			Text:   match.Text,
			Ranges: triples(match.Ranges),
			Widths: widths,
		}
		if match.Frame == nil || match.Frame.FrameHash != frame.FrameHash {
			t.Errorf("%s: match is not tied to the queried frame", name)
		}
	}

	if !sameJSON(t, observed, testCase.Expect.Matches) {
		t.Errorf("%s: matches %+v, want %+v", name, observed, testCase.Expect.Matches)
	}
}

func TestOfflineParityCorpusAndExactFrameFacts(t *testing.T) {
	parity := fixture[parityFixture](t, "conformance/locators/parity.json")
	data, err := os.ReadFile(
		filepath.Join(repo(t), "fixtures", "conformance", "locators", parity.Frame),
	)
	fatal(t, err)

	var frame termtrace.ScreenFrame
	fatal(t, termtrace.DecodeJSON(data, &frame))

	source, ok := frame.Source.(termtrace.ScreenFrameSourceActor)
	if !ok {
		t.Fatalf("source = %T", frame.Source)
	}

	if frame.StateHash != parity.FrameFacts["state_hash"] ||
		frame.FrameHash != parity.FrameFacts["frame_hash"] {
		t.Errorf("hashes = %s %s", frame.StateHash, frame.FrameHash)
	}

	if strconv.FormatUint(
		source.ObservedMonotonicNs,
		10,
	) != parity.FrameFacts["observed_monotonic_ns"] {
		t.Errorf("observed_monotonic_ns = %d", source.ObservedMonotonicNs)
	}

	if strconv.FormatUint(
		source.Boundaries.Observed.NextEventSeq,
		10,
	) != parity.FrameFacts["observed_next_event_seq"] {
		t.Errorf("observed next_event_seq = %d", source.Boundaries.Observed.NextEventSeq)
	}

	if frame.Cursor.Row != nil || frame.Cursor.Column != nil {
		t.Errorf("cursor = %+v", frame.Cursor)
	}

	client, _ := newClient(t)

	ctx := within(t, 30*time.Second)
	if len(parity.Cases) < 12 {
		t.Fatalf("only %d cases", len(parity.Cases))
	}

	for _, testCase := range parity.Cases {
		checkParityCase(t, ctx, client, frame, testCase)
	}
}

func TestLiveLocatorParityRichFramesAndWaits(t *testing.T) {
	parity := fixture[parityFixture](t, "conformance/locators/parity.json")
	live := parity.Live
	client, _ := newClient(t)
	ctx := within(t, 60*time.Second)
	recording := launch(
		t,
		client,
		live.Command,
		termtrace.LaunchOptions{Rows: new(live.Rows), Cols: new(live.Cols)},
	)
	screen := recording.Screen()

	ready, err := screen.GetByText(live.Ready).
		Wait(ctx, termtrace.LocatorWaitOptions{Timeout: 10 * time.Second, Interval: 20 * time.Millisecond})
	fatal(t, err)

	for _, testCase := range live.Cases {
		checkParityCase(t, ctx, client, ready.Frame, testCase)
	}

	frame, err := screen.Frame(ctx, termtrace.Projection{})
	fatal(t, err)

	spans, ok := frame.Data.(termtrace.ScreenFrameDataSpans)
	if frame.Geometry != (termtrace.ScreenGeometry{Rows: live.Rows, Cols: live.Cols}) ||
		!strings.Contains(frame.Text(), "END") ||
		!ok ||
		len(spans.Rows) != live.Rows {
		t.Fatalf("frame = %+v", frame.Geometry)
	}

	if _, ok := frame.Source.(termtrace.ScreenFrameSourceActor); !ok {
		t.Errorf("source = %T", frame.Source)
	}

	if !frame.Modes.Wraparound || len(frame.Colors.Palette) != 256 || !spans.Rows[5].SoftWrapped ||
		!spans.Rows[6].WrapContinuation {
		t.Errorf("modes, palette, or wraps differ")
	}

	region := termtrace.ScreenRegion{Top: 1, Left: 0, Bottom: 2, Right: 6}
	cells, err := screen.Frame(
		ctx,
		termtrace.Projection{Detail: termtrace.FrameDetailCells, Region: &region},
	)
	fatal(t, err)

	cellRows, ok := cells.Data.(termtrace.ScreenFrameDataCells)
	if cells.ReturnedRegion != region || !ok ||
		cellRows.Rows[0].Cells[0].WideKind != termtrace.WideKindWide ||
		cellRows.Rows[0].Cells[1].WideKind != termtrace.WideKindSpacerTail {
		t.Errorf("cells = %+v", cells.ReturnedRegion)
	}

	if _, err := screen.Frame(
		ctx,
		termtrace.Projection{Detail: termtrace.FrameDetailCells},
	); err == nil {
		t.Error("cells without a region succeeded")
	}

	retained, err := client.Query(ctx, frame, termtrace.Text("Save"))
	fatal(t, err)

	if retained.Frame.FrameHash != frame.FrameHash || len(retained.Matches) != 2 {
		t.Errorf("retained query = %d matches", len(retained.Matches))
	}

	state, err := screen.WaitForText(ctx, "END", 5*time.Second)
	fatal(t, err)

	if state.Frame.StateHash != state.Hash || state.Frame.Cursor.Row == nil ||
		*state.Frame.Cursor.Row != state.CursorRow {
		t.Errorf("resolved state = %+v", state)
	}

	if count, err := screen.GetByText("Save").Count(ctx); err != nil || count != 2 {
		t.Errorf("count = %d, %v", count, err)
	}

	if count, err := screen.GetByRegex(`Sa+ve`).Count(ctx); err != nil || count != 2 {
		t.Errorf("regex count = %d, %v", count, err)
	}

	two, err := screen.GetByText("Save").Wait(ctx, termtrace.LocatorWaitOptions{
		Condition: termtrace.LocatorConditionCount{Count: 2},
		Timeout:   5 * time.Second,
	})
	fatal(t, err)
	stable, err := screen.GetByText("Cancel").Wait(ctx, termtrace.LocatorWaitOptions{
		Timeout:   5 * time.Second,
		Interval:  20 * time.Millisecond,
		StableFor: 50 * time.Millisecond,
	})
	fatal(t, err)

	if len(two.Matches) != 2 || len(stable.Matches) != 1 {
		t.Errorf("waits = %d, %d matches", len(two.Matches), len(stable.Matches))
	}

	unmatched, err := screen.WaitFor(
		ctx,
		termtrace.ScreenPredicateContains{Text: "NEVER-SHOWN"},
		termtrace.ScreenWaitOptions{Timeout: 300 * time.Millisecond},
	)
	fatal(t, err)

	if unmatched.Matched || unmatched.Satisfied ||
		unmatched.Quality != termtrace.ScreenWaitQualityDeadline ||
		unmatched.Frame.StateHash == "" {
		t.Errorf("unmatched wait = %+v", unmatched)
	}

	matched, err := screen.WaitFor(
		ctx,
		termtrace.ScreenPredicateRowContains{Row: 7, Text: "END"},
		termtrace.ScreenWaitOptions{},
	)
	fatal(t, err)

	if !matched.Satisfied || matched.FirstMatchMonotonicNs == nil {
		t.Errorf("matched wait = %+v", matched)
	}

	capture, err := screen.CaptureStable(
		ctx,
		termtrace.CaptureStableOptions{StableFor: 50 * time.Millisecond, Timeout: 5 * time.Second},
	)
	fatal(t, err)

	if capture.Quality != termtrace.StableCaptureQualityStable ||
		len(capture.SemanticStateZstd) == 0 ||
		capture.StableForNs != 50_000_000 {
		t.Errorf("capture = %s %d", capture.Quality, capture.StableForNs)
	}

	observation, err := screen.Observe(ctx, termtrace.ObserveOptions{})
	fatal(t, err)

	if observation.Boundary.NextEventSeq == 0 {
		t.Errorf("observation boundary = %+v", observation.Boundary)
	}

	never := screen.GetByText("NEVER-SHOWN")
	for _, options := range []termtrace.LocatorWaitOptions{
		{Timeout: time.Second, StableFor: 2 * time.Second},
	} {
		if _, err := never.Wait(ctx, options); termtraceError(t, err).Kind() != "invalid_query" {
			t.Errorf("wait %+v = %v", options, err)
		}
	}

	started := time.Now()
	_, err = never.Wait(
		ctx,
		termtrace.LocatorWaitOptions{Timeout: time.Second, Interval: 20 * time.Millisecond},
	)

	failure := termtraceError(t, err)
	if failure.Kind() != "locator_timeout" || failure.Facts.Frame == nil ||
		time.Since(started) > 2*time.Second {
		t.Errorf("locator timeout = %v after %v", err, time.Since(started))
	}

	short, cancel := context.WithCancel(ctx)
	waiting := make(chan error, 1)

	go func() {
		_, err := never.Wait(
			short,
			termtrace.LocatorWaitOptions{
				Timeout:  30 * time.Second,
				Interval: 20 * time.Millisecond,
			},
		)
		waiting <- err
	}()

	time.Sleep(300 * time.Millisecond)

	during, err := screen.Frame(ctx, termtrace.Projection{})
	fatal(t, err)

	select {
	case err := <-waiting:
		t.Fatalf("the pending wait ended early: %v", err)
	default:
	}

	if !strings.Contains(during.Text(), "END") {
		t.Errorf("frame during a pending wait = %q", during.Text())
	}

	canceledAt := time.Now()

	cancel()

	if err := <-waiting; !errors.Is(err, context.Canceled) {
		t.Errorf("canceled wait = %v", err)
	}

	frameAfter, err := screen.Frame(ctx, termtrace.Projection{})
	fatal(t, err)

	latency := time.Since(canceledAt)
	if latency > 1500*time.Millisecond || !strings.Contains(frameAfter.Text(), "END") {
		t.Errorf("follow-up frame after cancellation took %v", latency)
	}

	if _, err := screen.FrameAt(
		ctx,
		termtrace.StatePointStateId{StateID: 0},
		termtrace.FrameAtOptions{},
	); err == nil {
		t.Error("a live session resolved a past state")
	}

	fatal(t, recording.Stop(ctx))
	completed, err := client.Attach(ctx, string(recording.Path()))
	fatal(t, err)
	first, err := completed.Screen().
		FrameAt(ctx, termtrace.StatePointStateId{StateID: 0}, termtrace.FrameAtOptions{IncludeHidden: true})
	fatal(t, err)

	resolved, ok := first.Source.(termtrace.ScreenFrameSourceResolved)
	if first.CaptureQuality != termtrace.CaptureQualityCompleted || !ok ||
		resolved.StateID == nil ||
		*resolved.StateID != 0 ||
		!resolved.SourceCompleted {
		t.Errorf("first state = %s %+v", first.CaptureQuality, first.Source)
	}

	observed, err := completed.Screen().
		Observe(ctx, termtrace.ObserveOptions{Point: termtrace.StatePointStateId{StateID: 0}, IncludeHidden: true})
	fatal(t, err)

	if observed.StateID == nil || *observed.StateID != 0 || observed.CanonicalSession == nil {
		t.Errorf("observation = %+v", observed)
	}

	final, err := completed.Screen().Frame(ctx, termtrace.Projection{})
	fatal(t, err)

	if !final.Presented || !strings.Contains(final.Text(), "END") {
		t.Errorf("final frame = %q", final.Text())
	}

	if count, err := completed.Screen().GetByText("Save").Count(ctx); err != nil || count != 2 {
		t.Errorf("completed count = %d, %v", count, err)
	}

	erasing := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", `printf LOADING; sleep 1; printf '\r\033[K'; sleep 30`},
		termtrace.LaunchOptions{Rows: new(5), Cols: new(40)},
	)
	loading := erasing.Screen().GetByText("LOADING")
	_, err = loading.Wait(ctx, termtrace.LocatorWaitOptions{Timeout: 5 * time.Second})
	fatal(t, err)
	gone, err := loading.Wait(
		ctx,
		termtrace.LocatorWaitOptions{
			Condition: termtrace.LocatorConditionAbsent,
			Timeout:   5 * time.Second,
			Interval:  20 * time.Millisecond,
		},
	)
	fatal(t, err)

	if len(gone.Matches) != 0 || strings.Contains(gone.Frame.Text(), "LOADING") {
		t.Errorf("absent wait = %d matches", len(gone.Matches))
	}
}
