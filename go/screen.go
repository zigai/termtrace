package termtrace

import (
	"context"
	"strings"
	"time"
)

// Screen reads and waits on the presented screen of a session.
type Screen struct {
	session *Session
}

// Projection selects the detail and region of a returned frame.
type Projection struct {
	// Detail is none, text, spans, or cells; spans when empty. Cells require a region.
	Detail FrameDetail
	// Region is a half-open region; the whole viewport when nil.
	Region *ScreenRegion
}

// FrameAtOptions selects a state and the projection of its frame.
type FrameAtOptions struct {
	Projection

	// IncludeHidden allows states that were never presented.
	IncludeHidden bool
}

// ObserveOptions selects the state to observe.
type ObserveOptions struct {
	// Point is the state point; the latest presented state when nil.
	Point StatePoint
	// IncludeHidden allows states that were never presented.
	IncludeHidden bool
}

// CaptureStableOptions bounds a stable capture.
type CaptureStableOptions struct {
	Projection

	// StableFor is how long the screen must stay unchanged; the default idle interval when zero.
	StableFor time.Duration
	// Timeout bounds the capture; the default wait timeout when zero.
	Timeout time.Duration
	// AllowIncomplete returns an unsettled capture instead of failing.
	AllowIncomplete bool
	// Reaction must happen before the stable interval starts.
	Reaction *ReactionSpec
}

// ScreenWaitOptions bounds a predicate wait.
type ScreenWaitOptions struct {
	// StableFor is how long the predicate must hold; none when zero.
	StableFor time.Duration
	// Timeout bounds the wait; the default wait timeout when zero.
	Timeout time.Duration
}

func (p Projection) wire() ProjectionSpec {
	return ProjectionSpec{Detail: optional(p.Detail), Region: p.Region}
}

// Frame returns the latest presented frame.
func (s *Screen) Frame(ctx context.Context, projection Projection) (ScreenFrame, error) {
	return s.session.client.screenFrame(
		ctx,
		frameParams{Session: s.session.ref(), Projection: projection.wire()},
	)
}

// FrameAt returns the frame at point of a completed session, or the latest presented frame of
// a live one.
func (s *Screen) FrameAt(
	ctx context.Context,
	point StatePoint,
	options FrameAtOptions,
) (ScreenFrame, error) {
	return s.session.client.screenFrameAt(ctx, frameAtParams{
		Session:       s.session.ref(),
		Point:         point,
		IncludeHidden: options.IncludeHidden,
		Projection:    options.wire(),
	})
}

// Observe returns the complete observation of a state.
func (s *Screen) Observe(ctx context.Context, options ObserveOptions) (Observation, error) {
	return s.session.client.screenObserve(ctx, observeParams{
		Session:       s.session.ref(),
		Point:         options.Point,
		IncludeHidden: options.IncludeHidden,
	})
}

// CaptureStable captures one exact frame once it stays unchanged.
func (s *Screen) CaptureStable(
	ctx context.Context,
	options CaptureStableOptions,
) (StableScreenCapture, error) {
	return s.session.client.screenCaptureStable(ctx, captureStableParams{
		Session:         s.session.ref(),
		StableForMs:     milliseconds(options.StableFor),
		TimeoutMs:       milliseconds(options.Timeout),
		Projection:      options.wire(),
		AllowIncomplete: options.AllowIncomplete,
		Reaction:        options.Reaction,
	})
}

// WaitFor waits for predicate on the presented screen. An unmatched outcome is a result, not
// an error; its quality tells a deadline from the child exiting.
func (s *Screen) WaitFor(
	ctx context.Context,
	predicate ScreenPredicate,
	options ScreenWaitOptions,
) (ScreenWaitResult, error) {
	return s.session.client.screenWaitFor(ctx, waitForParams{
		Session:     s.session.ref(),
		Predicate:   predicate,
		StableForMs: milliseconds(options.StableFor),
		TimeoutMs:   milliseconds(options.Timeout),
	})
}

// Text returns the plain text of the latest presented frame.
func (s *Screen) Text(ctx context.Context) (string, error) {
	return s.session.client.screenText(ctx, textParams{Session: s.session.ref()})
}

// WaitForText waits up to timeout, or the default wait timeout when zero, until the presented
// screen contains text. Not finding it is an error of kind "wait_unsatisfied".
func (s *Screen) WaitForText(
	ctx context.Context,
	text string,
	timeout time.Duration,
) (ResolvedState, error) {
	return s.session.client.screenWaitForText(ctx, waitForTextParams{
		Session:   s.session.ref(),
		Pattern:   text,
		TimeoutMs: milliseconds(timeout),
	})
}

// WaitForIdle waits up to timeout, or the default wait timeout when zero, until the presented
// screen stays unchanged for the default idle interval.
func (s *Screen) WaitForIdle(ctx context.Context, timeout time.Duration) (ResolvedState, error) {
	return s.session.client.screenWaitForIdle(
		ctx,
		waitForIdleParams{Session: s.session.ref(), TimeoutMs: milliseconds(timeout)},
	)
}

// Locate binds spec to this screen.
func (s *Screen) Locate(spec LocatorSpec) Locator {
	return Locator{screen: s, spec: spec}
}

// GetByText locates a nonempty literal substring.
func (s *Screen) GetByText(text string) Locator {
	return s.Locate(Text(text))
}

// GetByExactText locates a row or region whose complete text equals text.
func (s *Screen) GetByExactText(text string) Locator {
	return s.Locate(ExactText(text))
}

// GetByRegex locates text matching a Rust regex that cannot match empty text.
func (s *Screen) GetByRegex(pattern string) Locator {
	return s.Locate(Regex(pattern))
}

// Row locates every cell of one absolute row.
func (s *Screen) Row(row int) Locator {
	return s.Locate(LocatorSpec{}.InRow(row))
}

// Region locates every cell of one nonempty half-open region.
func (s *Screen) Region(region ScreenRegion) Locator {
	return s.Locate(LocatorSpec{}.InRegion(region))
}

// Filter locates cells with an appearance.
func (s *Screen) Filter(style StyleFilter) Locator {
	return s.Locate(LocatorSpec{}.WithStyle(style))
}

// Text returns the rows of the frame joined by newlines, or "" for a frame projected without
// detail.
func (f ScreenFrame) Text() string {
	var rows []string

	switch data := f.Data.(type) {
	case ScreenFrameDataText:
		for _, row := range data.Rows {
			rows = append(rows, row.Text)
		}
	case ScreenFrameDataSpans:
		for _, row := range data.Rows {
			rows = append(rows, row.Text)
		}
	case ScreenFrameDataCells:
		for _, row := range data.Rows {
			rows = append(rows, row.Text)
		}
	}

	return strings.Join(rows, "\n")
}
