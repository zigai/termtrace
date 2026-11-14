package termtrace

import (
	"context"
	"time"
)

// Output reads the raw bytes a session's child wrote. It reads the session directory, so it
// also works after the recording stops.
type Output struct {
	client *Client
	path   Path
}

// OutputPageOptions selects one page of output.
//
//exhaustruct:optional
type OutputPageOptions struct {
	// Cursor is the position the previous page returned; the start when nil.
	Cursor *OutputPageCursor
	// Limit caps the bytes returned; 65536 when zero.
	Limit int
}

// Events reads the canonical event log of a session.
type Events struct {
	client *Client
	path   Path
}

// EventPageOptions selects and filters one page of events.
//
//exhaustruct:optional
type EventPageOptions struct {
	// Cursor is the position the previous page returned; the start when nil.
	Cursor *EventPageCursor
	// Limit caps the events returned; 100 when zero.
	Limit int
	// MetadataOnly leaves out event payloads.
	MetadataOnly bool
	// EventTypes keeps only these event types; every type when empty.
	EventTypes []string
}

// Analysis replays a session to explain, compare, and review its states.
type Analysis struct {
	client *Client
	path   Path
}

// TimelineOptions selects one timeline page.
//
//exhaustruct:optional
type TimelineOptions struct {
	// Offset is the index of the first entry.
	Offset uint64
	// Limit caps the entries returned; 100 when zero.
	Limit int
	// IncludeHidden includes states that were never presented.
	IncludeHidden bool
	// Controls includes control entries.
	Controls bool
	// Range keeps entries inside a session-monotonic range.
	Range *SourceRange
}

// ExplainOptions selects the state to explain.
//
//exhaustruct:optional
type ExplainOptions struct {
	// Target is the state to explain; the latest presented state when nil.
	Target StatePoint
	// Context is the number of events of context; 8 when zero.
	Context uint64
}

// DiagnoseOptions selects diagnostic checks.
//
//exhaustruct:optional
type DiagnoseOptions struct {
	// Checks are the checks to run; every check when empty.
	Checks []DiagnosticCheck
	// BroadChangePercent is the changed-cell percent that counts as a broad change; 50 when
	// zero.
	BroadChangePercent int
}

// Evidence writes artifacts and bundles from a session.
type Evidence struct {
	client *Client
	path   Path
}

// SaveOptions selects the state to save.
//
//exhaustruct:optional
type SaveOptions struct {
	// Point is the state to save; the latest presented state when nil.
	Point StatePoint
	// IncludeHidden lets Point select a state that was never presented.
	IncludeHidden bool
}

// Integrity verifies, signs, and recovers session evidence.
type Integrity struct {
	client *Client
	path   Path
}

// VerifyOptions sets the verification depth and trust anchor.
//
//exhaustruct:optional
type VerifyOptions struct {
	// Level is the verification depth; content when empty.
	Level VerificationLevel
	// AllowedSigners is an allowed-signers file that decides trust.
	AllowedSigners string
}

// Output returns the raw output reader of the session.
func (s *Session) Output() *Output {
	return s.client.Output(string(s.path))
}

// Events returns the event log reader of the session.
func (s *Session) Events() *Events {
	return s.client.Events(string(s.path))
}

// Analysis returns the replay analysis of the session.
func (s *Session) Analysis() *Analysis {
	return s.client.Analysis(string(s.path))
}

// Evidence returns the artifact writer of the session.
func (s *Session) Evidence() *Evidence {
	return s.client.Evidence(string(s.path))
}

// Integrity returns the integrity operations of the session.
func (s *Session) Integrity() *Integrity {
	return s.client.Integrity(string(s.path))
}

// Output returns the raw output reader of the session directory at path.
func (c *Client) Output(path string) *Output {
	return &Output{client: c, path: Path(path)}
}

// Events returns the event log reader of the session directory at path.
func (c *Client) Events(path string) *Events {
	return &Events{client: c, path: Path(path)}
}

// Analysis returns the replay analysis of the session directory at path.
func (c *Client) Analysis(path string) *Analysis {
	return &Analysis{client: c, path: Path(path)}
}

// Evidence returns the artifact writer of the session directory at path.
func (c *Client) Evidence(path string) *Evidence {
	return &Evidence{client: c, path: Path(path)}
}

// Integrity returns the integrity operations of the session directory or event log at path.
func (c *Client) Integrity(path string) *Integrity {
	return &Integrity{client: c, path: Path(path)}
}

// Page reads one page of raw output bytes.
func (o *Output) Page(ctx context.Context, options OutputPageOptions) (ByteOutputPage, error) {
	return o.client.outputPage(ctx, outputPageParams{
		Session: o.path,
		Cursor:  options.Cursor,
		Limit:   positive(options.Limit),
	})
}

// WaitFor waits until the output contains pattern, up to timeout, or the server default when
// timeout is zero.
func (o *Output) WaitFor(
	ctx context.Context,
	pattern []byte,
	timeout time.Duration,
) (WaitOutcome, error) {
	return o.client.outputWaitFor(ctx, outputWaitParams{
		Session:   o.path,
		Pattern:   pattern,
		TimeoutMs: milliseconds(timeout),
	})
}

// Page reads one page of events.
func (e *Events) Page(ctx context.Context, options EventPageOptions) (ProjectedEventPage, error) {
	return e.client.eventsPage(ctx, eventsPageParams{
		Session:      e.path,
		Cursor:       options.Cursor,
		Limit:        positive(options.Limit),
		MetadataOnly: options.MetadataOnly,
		EventTypes:   options.EventTypes,
	})
}

// Follow waits until the event log reaches nextEventSeq and reports its position.
func (e *Events) Follow(ctx context.Context, nextEventSeq uint64) (EventFollowPosition, error) {
	return e.client.eventsFollow(ctx, eventsFollowParams{
		Session:      e.path,
		NextEventSeq: new(nextEventSeq),
	})
}

// Timeline reads one timeline page.
func (a *Analysis) Timeline(ctx context.Context, options TimelineOptions) (TimelinePage, error) {
	return a.client.analysisTimeline(ctx, timelineParams{
		Session:       a.path,
		Offset:        options.Offset,
		Limit:         positive(options.Limit),
		IncludeHidden: options.IncludeHidden,
		Controls:      options.Controls,
		Range:         options.Range,
	})
}

// Diff compares two states.
func (a *Analysis) Diff(ctx context.Context, from StatePoint, to StatePoint) (SemanticDiff, error) {
	return a.client.analysisDiff(
		ctx,
		diffParams{Session: a.path, From: from, To: to},
	)
}

// Explain explains one state from the bytes that produced it.
func (a *Analysis) Explain(ctx context.Context, options ExplainOptions) (ExplainReport, error) {
	var events *uint64
	if options.Context > 0 {
		events = new(options.Context)
	}

	return a.client.analysisExplain(ctx, explainParams{
		Session: a.path,
		Target:  options.Target,
		Context: events,
	})
}

// Diagnose runs conservative diagnostics.
func (a *Analysis) Diagnose(ctx context.Context, options DiagnoseOptions) (DiagnoseReport, error) {
	return a.client.analysisDiagnose(ctx, diagnoseParams{
		Session:            a.path,
		Checks:             options.Checks,
		BroadChangePercent: positive(options.BroadChangePercent),
	})
}

// LastAction triages the most recent action. broadChangePercent is the changed-cell percent
// that counts as a broad change; 50 when zero.
func (a *Analysis) LastAction(
	ctx context.Context,
	broadChangePercent int,
) (LastActionAnalysis, error) {
	return a.client.analysisLastAction(ctx, lastActionParams{
		Session:            a.path,
		BroadChangePercent: positive(broadChangePercent),
	})
}

// Markers lists the session markers with their frame numbers at fps, or 30 when fps is zero.
func (a *Analysis) Markers(ctx context.Context, fps int) ([]MarkerRecord, error) {
	return a.client.analysisMarkers(ctx, markersParams{
		Session: a.path,
		FPS:     positive(fps),
	})
}

// CacheStatus inventories the replay cache.
func (a *Analysis) CacheStatus(ctx context.Context) (ReplayCacheStatus, error) {
	return a.client.analysisCacheStatus(ctx, sessionParams{Session: a.path})
}

// CachePrune removes disposable replay checkpoints.
func (a *Analysis) CachePrune(ctx context.Context) (ReplayCachePruneReport, error) {
	return a.client.analysisCachePrune(ctx, sessionParams{Session: a.path})
}

// ReviewSummary summarizes the session for review.
func (a *Analysis) ReviewSummary(ctx context.Context) (ReviewSummary, error) {
	return a.client.analysisReviewSummary(ctx, sessionParams{Session: a.path})
}

// ReviewSearch searches the presented screens for query.
func (a *Analysis) ReviewSearch(ctx context.Context, query string) (ReviewSearchResult, error) {
	return a.client.analysisReviewSearch(ctx, reviewSearchParams{
		Session: a.path,
		Query:   query,
	})
}

// ReviewExport writes one reviewed state to a new destination directory.
func (a *Analysis) ReviewExport(
	ctx context.Context,
	stateID uint64,
	destination string,
) (ReviewExportManifest, error) {
	return a.client.analysisReviewExport(ctx, reviewExportParams{
		Session:     a.path,
		StateID:     stateID,
		Destination: Path(destination),
	})
}

// Save writes the requested representations of one state to destination.
func (e *Evidence) Save(
	ctx context.Context,
	destination string,
	formats []SaveFormat,
	options SaveOptions,
) (SaveOutcome, error) {
	return e.client.evidenceSave(ctx, saveParams{
		Session:       e.path,
		Destination:   Path(destination),
		Formats:       formats,
		Point:         options.Point,
		IncludeHidden: options.IncludeHidden,
	})
}

// Bundle writes an evidence bundle with profile to the new directory destination.
func (e *Evidence) Bundle(
	ctx context.Context,
	destination string,
	profile BundleProfile,
) (BundleManifest, error) {
	return e.client.evidenceBundle(ctx, bundleParams{
		Session: e.path,
		Bundle:  Path(destination),
		Profile: profile,
	})
}

// VerifyBundle checks the bundle in directory against this session and profile.
func (e *Evidence) VerifyBundle(
	ctx context.Context,
	directory string,
	profile BundleProfile,
) (BundleManifest, error) {
	return e.client.evidenceVerifyBundle(ctx, bundleParams{
		Session: e.path,
		Bundle:  Path(directory),
		Profile: profile,
	})
}

// Asciicast exports the session as an asciicast file and returns its path.
func (e *Evidence) Asciicast(ctx context.Context, destination string) (Path, error) {
	return e.client.evidenceAsciicast(ctx, exportParams{
		Session:     e.path,
		Destination: Path(destination),
	})
}

// Verify checks the session evidence. Validity, authenticity, and trust are separate facts of
// the report.
func (i *Integrity) Verify(ctx context.Context, options VerifyOptions) (VerificationReport, error) {
	params := verifyParams{Session: i.path}
	if options.Level != "" {
		params.Level = new(options.Level)
	}

	if options.AllowedSigners != "" {
		params.AllowedSigners = new(Path(options.AllowedSigners))
	}

	return i.client.integrityVerify(ctx, params)
}

// Sign writes a verification receipt signed with the SSH key at key and returns its path.
// Signing never implies trust.
func (i *Integrity) Sign(ctx context.Context, key string, signer string) (Path, error) {
	return i.client.integritySign(ctx, signParams{
		Session: i.path,
		Key:     Path(key),
		Signer:  signer,
	})
}

// Recover copies the valid prefix of the session into the new directory destination.
func (i *Integrity) Recover(ctx context.Context, destination string) (RecoveryReport, error) {
	return i.client.integrityRecover(ctx, exportParams{
		Session:     i.path,
		Destination: Path(destination),
	})
}

// RecoverLog copies the valid prefix of the event log into the new log destination.
func (i *Integrity) RecoverLog(ctx context.Context, destination string) (RecoveryReport, error) {
	return i.client.integrityRecoverLog(ctx, exportParams{
		Session:     i.path,
		Destination: Path(destination),
	})
}

func positive(value int) *int {
	if value <= 0 {
		return nil
	}

	return new(value)
}
