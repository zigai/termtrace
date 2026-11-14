package termtrace

import (
	"context"
	"errors"
	"fmt"
	"math"
	"slices"
	"time"
)

var (
	errNegativeIndex = errors.New("termtrace: match index is negative")
	errCountOverflow = errors.New("termtrace: match count overflows int")
)

// LocatorSpec is an immutable screen query. The zero value matches every cell. Invalid specs,
// such as empty text or a regex that can match empty text, fail when evaluated with kind
// "invalid_query".
type LocatorSpec struct {
	clauses []locatorClause //exhaustruct:optional
}

// ScreenMatch is one match and the frame it was found in.
type ScreenMatch struct {
	LocatorMatch

	// Frame is the exact frame that produced the match.
	Frame *ScreenFrame

	client *Client
	spec   LocatorSpec
	index  int
}

// QueryResult is every match of one evaluation.
type QueryResult struct {
	// Frame is the frame the query ran on.
	Frame ScreenFrame
	// Matches are in screen order.
	Matches []ScreenMatch

	client *Client
	spec   LocatorSpec
}

// Locator is a [LocatorSpec] bound to a screen. Each evaluation captures the latest presented
// frame.
type Locator struct {
	screen *Screen
	spec   LocatorSpec
}

// LocatorWaitOptions bounds a locator wait.
type LocatorWaitOptions struct {
	// Condition is the required match count; at least one match when nil.
	Condition LocatorCondition
	// Timeout bounds the wait; the default wait timeout when zero.
	Timeout time.Duration
	// Interval is the polling interval; the default when zero.
	Interval time.Duration
	// StableFor is how long the condition must hold; none when zero.
	StableFor time.Duration
}

// LocatorWaitResult is the frame that satisfied a wait and its matches.
type LocatorWaitResult struct {
	// Frame satisfied the condition.
	Frame ScreenFrame
	// Matches are in screen order.
	Matches []ScreenMatch
}

// Text matches a nonempty literal substring.
func Text(text string) LocatorSpec {
	return LocatorSpec{}.with(locatorClauseText{Text: text})
}

// ExactText matches a row or region whose complete text equals text.
func ExactText(text string) LocatorSpec {
	return LocatorSpec{}.with(locatorClauseExact{Text: text})
}

// Regex matches a Rust regex that cannot match empty text.
func Regex(pattern string) LocatorSpec {
	return LocatorSpec{}.with(locatorClauseRegex{Pattern: pattern})
}

// InRow restricts the query to one absolute row.
func (s LocatorSpec) InRow(row int) LocatorSpec {
	return s.with(locatorClauseRow{Row: row})
}

// InRegion restricts the query to one nonempty half-open region.
func (s LocatorSpec) InRegion(region ScreenRegion) LocatorSpec {
	return s.with(locatorClauseRegion{Region: region})
}

// WithStyle requires a cell appearance.
func (s LocatorSpec) WithStyle(style StyleFilter) LocatorSpec {
	return s.with(locatorClauseStyle{Style: style})
}

// AcrossSoftWraps lets text matches cross linked soft-wrapped rows.
func (s LocatorSpec) AcrossSoftWraps(enabled bool) LocatorSpec {
	return s.with(locatorClauseAcrossSoftWraps{Enabled: enabled})
}

// And combines independent scope, text, and appearance clauses. Two text matchers, conflicting
// rows, or disjoint regions fail when evaluated.
func (s LocatorSpec) And(other LocatorSpec) LocatorSpec {
	return s.with(locatorClauseAnd{Locator: other.wire()})
}

func (s LocatorSpec) with(clause locatorClause) LocatorSpec {
	return LocatorSpec{clauses: append(slices.Clip(s.clauses), clause)}
}

func (s LocatorSpec) wire() locatorQuery {
	return locatorQuery{Clauses: slices.Clone(s.clauses)}
}

// Query runs spec on frame.
func (c *Client) Query(
	ctx context.Context,
	frame ScreenFrame,
	spec LocatorSpec,
) (QueryResult, error) {
	found, err := c.frameQuery(ctx, frameQueryParams{Frame: frame, Locator: spec.wire()})
	if err != nil {
		return QueryResult{}, err
	}

	return newQueryResult(c, spec, LocatorMatches{Frame: frame, Matches: found}), nil
}

// Single returns the only match; none or several fail with kind "no_match" or
// "multiple_matches".
func (r QueryResult) Single(ctx context.Context) (ScreenMatch, error) {
	return r.selectMatch(ctx, selectionSingle{}, 0)
}

// First returns the first match; none fails with kind "no_match".
func (r QueryResult) First(ctx context.Context) (ScreenMatch, error) {
	return r.selectMatch(ctx, selectionFirst{}, 0)
}

// Nth returns the match at a zero-based index.
func (r QueryResult) Nth(ctx context.Context, index int) (ScreenMatch, error) {
	position, err := matchIndex(index)
	if err != nil {
		return ScreenMatch{}, err
	}

	return r.selectMatch(ctx, selectionNth{Index: position}, index)
}

func (r QueryResult) selectMatch(
	ctx context.Context,
	choice selection,
	index int,
) (ScreenMatch, error) {
	found, err := r.client.frameQuery(
		ctx,
		frameQueryParams{Frame: r.Frame, Locator: r.spec.wire(), Selection: choice},
	)
	if err != nil {
		return ScreenMatch{}, err
	}

	if len(found) != 1 {
		return ScreenMatch{}, &Error{Message: "server returned no selected match"}
	}

	return ScreenMatch{
		LocatorMatch: found[0],
		Frame:        &r.Frame,
		client:       r.client,
		spec:         r.spec,
		index:        index,
	}, nil
}

// Spec returns the unbound query.
func (l Locator) Spec() LocatorSpec {
	return l.spec
}

// InRow restricts the query to one absolute row.
func (l Locator) InRow(row int) Locator {
	return Locator{screen: l.screen, spec: l.spec.InRow(row)}
}

// InRegion restricts the query to one nonempty half-open region.
func (l Locator) InRegion(region ScreenRegion) Locator {
	return Locator{screen: l.screen, spec: l.spec.InRegion(region)}
}

// WithStyle requires a cell appearance.
func (l Locator) WithStyle(style StyleFilter) Locator {
	return Locator{screen: l.screen, spec: l.spec.WithStyle(style)}
}

// AcrossSoftWraps lets text matches cross linked soft-wrapped rows.
func (l Locator) AcrossSoftWraps(enabled bool) Locator {
	return Locator{screen: l.screen, spec: l.spec.AcrossSoftWraps(enabled)}
}

// And combines independent scope, text, and appearance clauses.
func (l Locator) And(other LocatorSpec) Locator {
	return Locator{screen: l.screen, spec: l.spec.And(other)}
}

// Evaluate captures the latest presented frame and returns every match.
func (l Locator) Evaluate(ctx context.Context) (QueryResult, error) {
	found, err := l.client().locatorEvaluate(ctx, l.params())
	if err != nil {
		return QueryResult{}, err
	}

	return newQueryResult(l.client(), l.spec, found), nil
}

// All returns every match in the latest presented frame.
func (l Locator) All(ctx context.Context) ([]ScreenMatch, error) {
	result, err := l.Evaluate(ctx)
	return result.Matches, err
}

// Count counts the matches in the latest presented frame.
func (l Locator) Count(ctx context.Context) (int, error) {
	count, err := l.client().locatorCount(ctx, l.params())
	if err != nil {
		return 0, err
	}

	if count > math.MaxInt {
		return 0, fmt.Errorf("%w: %d", errCountOverflow, count)
	}

	return int(count), nil
}

// Single returns the only match; none or several fail with kind "no_match" or
// "multiple_matches".
func (l Locator) Single(ctx context.Context) (ScreenMatch, error) {
	return l.selectMatch(ctx, selectionSingle{}, 0)
}

// First returns the first match; none fails with kind "no_match".
func (l Locator) First(ctx context.Context) (ScreenMatch, error) {
	return l.selectMatch(ctx, selectionFirst{}, 0)
}

// Nth returns the match at a zero-based index.
func (l Locator) Nth(ctx context.Context, index int) (ScreenMatch, error) {
	position, err := matchIndex(index)
	if err != nil {
		return ScreenMatch{}, err
	}

	return l.selectMatch(ctx, selectionNth{Index: position}, index)
}

// Wait polls until the match count satisfies the condition. Canceling ctx cancels the wait on
// the server; a timeout or cancellation fails with kind "locator_timeout" or "cancelled" and
// carries the last frame seen.
//
//nolint:misspell // "cancelled" is the wire kind the server reports for a canceled locator wait.
func (l Locator) Wait(ctx context.Context, options LocatorWaitOptions) (LocatorWaitResult, error) {
	found, err := l.client().locatorWaitFor(ctx, locatorWaitParams{
		Session:     l.screen.session.ref(),
		Locator:     l.spec.wire(),
		Condition:   options.Condition,
		TimeoutMs:   milliseconds(options.Timeout),
		IntervalMs:  milliseconds(options.Interval),
		StableForMs: milliseconds(options.StableFor),
	})
	if err != nil {
		return LocatorWaitResult{}, err
	}

	return LocatorWaitResult{
		Frame:   found.Frame,
		Matches: matches(l.client(), l.spec, &found.Frame, found.Matches),
	}, nil
}

func (l Locator) params() locatorParams {
	return locatorParams{Session: l.screen.session.ref(), Locator: l.spec.wire()}
}

func (l Locator) client() *Client {
	return l.screen.session.client
}

func (l Locator) selectMatch(
	ctx context.Context,
	choice selection,
	index int,
) (ScreenMatch, error) {
	found, err := l.client().
		locatorSelect(ctx, selectParams{Session: l.screen.session.ref(), Locator: l.spec.wire(), Selection: choice})
	if err != nil {
		return ScreenMatch{}, err
	}

	return ScreenMatch{
		LocatorMatch: found.Match,
		Frame:        &found.Frame,
		client:       l.client(),
		spec:         l.spec,
		index:        index,
	}, nil
}

func newQueryResult(client *Client, spec LocatorSpec, found LocatorMatches) QueryResult {
	return QueryResult{
		Frame:   found.Frame,
		Matches: matches(client, spec, &found.Frame, found.Matches),
		client:  client,
		spec:    spec,
	}
}

func matches(
	client *Client,
	spec LocatorSpec,
	frame *ScreenFrame,
	found []LocatorMatch,
) []ScreenMatch {
	out := make([]ScreenMatch, len(found))
	for index, match := range found {
		out[index] = ScreenMatch{
			LocatorMatch: match,
			Frame:        frame,
			client:       client,
			spec:         spec,
			index:        index,
		}
	}

	return out
}

func matchIndex(index int) (uint64, error) {
	if index < 0 {
		return 0, fmt.Errorf("%w: %d", errNegativeIndex, index)
	}

	return uint64(index), nil
}
