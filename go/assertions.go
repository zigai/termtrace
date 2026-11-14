package termtrace

import (
	"context"
	"errors"
	"time"
)

// SnapshotOptions sets what an unmasked snapshot includes.
//
//exhaustruct:optional
type SnapshotOptions struct {
	// Profile is the snapshot detail; text when empty.
	Profile SnapshotProfile
	// IncludeCursor adds cursor facts.
	IncludeCursor bool
	// IncludeModes adds mode facts.
	IncludeModes bool
}

// SnapshotFileOptions sets what a snapshot baseline file holds and whether to replace it.
//
//exhaustruct:optional
type SnapshotFileOptions struct {
	SnapshotOptions

	// Update creates or replaces the baseline instead of comparing with it.
	Update bool
}

// FinalizeOptions sets how [Client.FinalizeRecordings] ends a test.
//
//exhaustruct:optional
type FinalizeOptions struct {
	// Policy decides when evidence is kept; on failure when empty.
	Policy EvidencePolicy
	// Timeout bounds stopping and verifying each recording; 10 seconds when zero.
	Timeout time.Duration
}

// IsExpectationMismatch reports whether err is a valid expectation that did not match. Query,
// validation, and transport failures are not mismatches.
func IsExpectationMismatch(err error) bool {
	var failure *Error

	return errors.As(err, &failure) && failure.Kind() == "expectation_mismatch"
}

// AssertNot runs expectation and succeeds only when it fails with an expectation mismatch.
// Other failures return unchanged; an expectation that holds fails with a mismatch.
func (c *Client) AssertNot(ctx context.Context, expectation func() error) error {
	err := expectation()
	if err == nil {
		return c.testingFailNegation(ctx, empty{})
	}

	if IsExpectationMismatch(err) {
		return nil
	}

	return err
}

// NormalizedSnapshot normalizes frame into a snapshot, without observing or retrying.
func (c *Client) NormalizedSnapshot(
	ctx context.Context,
	frame ScreenFrame,
	options SnapshotOptions,
) (string, error) {
	return c.testingNormalizedSnapshot(ctx, normalizedSnapshotParams{
		Frame:         frame,
		Profile:       optional(options.Profile),
		IncludeCursor: options.IncludeCursor,
		IncludeModes:  options.IncludeModes,
	})
}

// MaskedSnapshot normalizes frame into a masked cells snapshot. Masks are not redaction.
func (c *Client) MaskedSnapshot(
	ctx context.Context,
	frame ScreenFrame,
	options MaskedSnapshotOptions,
) (string, error) {
	return c.testingMaskedSnapshot(ctx, maskedSnapshotParams{Frame: frame, Options: options})
}

// AssertSnapshotFile compares frame with the baseline file at path. A missing or different
// baseline is a mismatch; only options.Update writes it.
func (c *Client) AssertSnapshotFile(
	ctx context.Context,
	frame ScreenFrame,
	path string,
	options SnapshotFileOptions,
) error {
	return c.testingAssertSnapshotFile(ctx, snapshotFileParams{
		Frame:         frame,
		Path:          Path(path),
		Profile:       optional(options.Profile),
		IncludeCursor: options.IncludeCursor,
		IncludeModes:  options.IncludeModes,
		Update:        options.Update,
	})
}

// AssertMaskedSnapshotFile compares the masked snapshot of frame with the baseline file at
// path; update writes it instead.
func (c *Client) AssertMaskedSnapshotFile(
	ctx context.Context,
	frame ScreenFrame,
	path string,
	options MaskedSnapshotOptions,
	update bool,
) error {
	return c.testingAssertMaskedSnapshotFile(ctx, maskedSnapshotFileParams{
		Frame:   frame,
		Path:    Path(path),
		Options: options,
		Update:  update,
	})
}

// AssertScreenText requires literal text in the presented rows of frame.
func (c *Client) AssertScreenText(ctx context.Context, frame ScreenFrame, expected string) error {
	return c.testingAssertScreenText(ctx, screenTextParams{Frame: frame, Expected: expected})
}

// AssertCursor requires the cursor of frame.
func (c *Client) AssertCursor(ctx context.Context, frame ScreenFrame, expected CursorState) error {
	return c.testingAssertCursor(ctx, cursorParams{Frame: frame, Expected: expected})
}

// AssertNoWrappedRows requires that no row of frame soft-wraps.
func (c *Client) AssertNoWrappedRows(ctx context.Context, frame ScreenFrame) error {
	return c.testingAssertNoWrappedRows(ctx, frameAssertionParams{Frame: frame})
}

// AssertNeutralRegion requires that region of frame holds only default-styled blanks.
func (c *Client) AssertNeutralRegion(
	ctx context.Context,
	frame ScreenFrame,
	region ScreenRegion,
) error {
	return c.testingAssertNeutralRegion(ctx, neutralRegionParams{Frame: frame, Region: region})
}

// AssertNoPresentedTransient requires that frame was not presented mid-update.
func (c *Client) AssertNoPresentedTransient(ctx context.Context, frame ScreenFrame) error {
	return c.testingAssertNoPresentedTransient(ctx, frameAssertionParams{Frame: frame})
}

// AssertExists requires at least one match.
func (r QueryResult) AssertExists(ctx context.Context) error {
	return r.client.testingAssertExists(ctx, r.queryParams())
}

// AssertNotExists requires no match.
func (r QueryResult) AssertNotExists(ctx context.Context) error {
	return r.client.testingAssertNotExists(ctx, r.queryParams())
}

// AssertCount requires exactly expected matches.
func (r QueryResult) AssertCount(ctx context.Context, expected int) error {
	count, err := matchIndex(expected)
	if err != nil {
		return err
	}

	return r.client.testingAssertCount(ctx, countParams{
		Frame:    r.Frame,
		Locator:  r.spec.wire(),
		Expected: count,
	})
}

func (r QueryResult) queryParams() queryParams {
	return queryParams{Frame: r.Frame, Locator: r.spec.wire()}
}

// AssertText requires the matched text to equal expected.
func (m ScreenMatch) AssertText(ctx context.Context, expected string) error {
	index, err := matchIndex(m.index)
	if err != nil {
		return err
	}

	return m.client.testingAssertMatchText(ctx, matchTextParams{
		Frame:    *m.Frame,
		Locator:  m.spec.wire(),
		Index:    index,
		Expected: expected,
	})
}

// AssertStyle requires every covered cell to have the appearance expected.
func (m ScreenMatch) AssertStyle(ctx context.Context, expected StyleFilter) error {
	index, err := matchIndex(m.index)
	if err != nil {
		return err
	}

	return m.client.testingAssertMatchStyle(ctx, matchStyleParams{
		Frame:    *m.Frame,
		Locator:  m.spec.wire(),
		Index:    index,
		Expected: expected,
	})
}

// FinalizeRecordings stops, verifies, and keeps or discards the recordings a test owns under
// root, following outcome and options.Policy. Every recording is attempted; cleanup failures
// are in the report. Each recording is released, so call it once per test.
func (c *Client) FinalizeRecordings(
	ctx context.Context,
	recordings []*Recording,
	root string,
	outcome TestOutcome,
	options FinalizeOptions,
) (FinalizationReport, error) {
	handles := make([]string, len(recordings))
	for index, recording := range recordings {
		handles[index] = recording.handle
	}

	return c.testingFinalize(ctx, finalizeParams{
		Recordings: handles,
		Root:       Path(root),
		Outcome:    outcome,
		Policy:     options.Policy,
		TimeoutMs:  milliseconds(options.Timeout),
	})
}
