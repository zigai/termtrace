package termtrace

import (
	"context"
	"time"
)

const editPlanVersion = 2

// Edit builds a presentation edit plan. It only holds values; [Client.NormalizeEdit] and the
// video calls validate it.
type Edit struct {
	plan         EditPlan
	resourceBase *Path //exhaustruct:optional
}

// ClipOptions sets the speed, end hold, and caption of one clip.
//
//exhaustruct:optional
type ClipOptions struct {
	// Speed is the playback rate; 1/1 when zero.
	Speed RationalSpeed
	// Hold keeps the last frame on screen this long.
	Hold time.Duration
	// Caption is shown while the clip plays.
	Caption string
}

// NativeScreenshotOptions selects the state and startup bound of a native screenshot.
//
//exhaustruct:optional
type NativeScreenshotOptions struct {
	// Point is the state to capture; the latest presented state when nil.
	Point StatePoint
	// Timeout bounds the replay terminal's startup; 30 seconds when zero.
	Timeout time.Duration
}

// ScenarioSuiteOptions selects, parallelizes, and reruns scenario cases.
//
//exhaustruct:optional
type ScenarioSuiteOptions struct {
	// FailureRoot holds failure artifacts; snapshot baselines live in its sibling snapshots.
	FailureRoot string
	// EvidenceRoot holds each case's session; the client's evidence root when empty.
	EvidenceRoot string
	// Jobs is the number of cases run at once; 1 when zero.
	Jobs int
	// Shard selects case i when i % Shard.Count == Shard.Index.
	Shard *ScenarioShard
	// UpdateSnapshots replaces snapshot baselines instead of comparing them.
	UpdateSnapshots bool
	// Reruns is the number of fresh-session reruns of a failed case.
	Reruns uint8
	// FailOnFlaky makes a flaky case fail CI.
	FailOnFlaky bool
}

// ScenarioShard is one shard of a scenario suite.
type ScenarioShard struct {
	// Index is the zero-based shard index.
	Index int
	// Count is the total number of shards.
	Count int
}

// NewEdit returns an empty edit plan.
func NewEdit() *Edit {
	return &Edit{plan: EditPlan{Version: editPlanVersion, Clips: []EditClip{}}}
}

// SelectMarker selects the state at a unique marker label.
func SelectMarker(label string) EditSelector {
	return EditSelector{Marker: new(label)}
}

// SelectMonotonicNs selects the state at an exact session offset in nanoseconds.
func SelectMonotonicNs(offset uint64) EditSelector {
	return EditSelector{MonotonicNs: new(offset)}
}

// SelectEvent selects the latest state at or before a canonical event.
func SelectEvent(seq uint64) EditSelector {
	return EditSelector{EventSeq: new(seq)}
}

// SelectState selects a state by its stream position.
func SelectState(id uint64) EditSelector {
	return EditSelector{StateId: new(id)}
}

// SelectNativeFrame selects an exact native source frame.
func SelectNativeFrame(frame uint64) EditSelector {
	return EditSelector{NativeFrame: new(frame)}
}

// WithResourceBase resolves relative subtitle paths against base.
func (e *Edit) WithResourceBase(base string) *Edit {
	e.resourceBase = new(Path(base))

	return e
}

// Clip adds the span from one selected state to another.
func (e *Edit) Clip(from EditSelector, to EditSelector, options ClipOptions) *Edit {
	clip := EditClip{From: from, To: to, Speed: options.Speed}
	if options.Hold > 0 {
		clip.HoldMs = uint64(options.Hold / time.Millisecond)
	}

	if options.Caption != "" {
		clip.Caption = new(options.Caption)
	}

	e.plan.Clips = append(e.plan.Clips, clip)

	return e
}

// Annotate adds a text annotation.
func (e *Edit) Annotate(annotation TextAnnotation) *Edit {
	e.plan.Annotations = append(e.plan.Annotations, annotation)

	return e
}

// Subtitles adds an ASS subtitle track.
func (e *Edit) Subtitles(ass string) *Edit {
	e.plan.Subtitles = append(e.plan.Subtitles, SubtitleTrack{Ass: Path(ass)})

	return e
}

// Changes highlights screen changes.
func (e *Edit) Changes(visualization ChangeVisualization) *Edit {
	e.plan.ChangeVisualization = &visualization

	return e
}

// CaptionLayout places clip captions.
func (e *Edit) CaptionLayout(caption CaptionLayout) *Edit {
	e.plan.Layout.Caption = caption

	return e
}

// Branding shows label on every frame.
func (e *Edit) Branding(label string) *Edit {
	if e.plan.Layout.Caption == "" {
		e.plan.Layout.Caption = CaptionLayoutFooter
	}

	e.plan.Layout.Branding = new(label)

	return e
}

// Delivery sets the encoder settings.
func (e *Edit) Delivery(delivery PresentationDelivery) *Edit {
	e.plan.Delivery = delivery

	return e
}

// CursorMask hides the cursor.
func (e *Edit) CursorMask() *Edit {
	return e.Transform(PixelTransformCursorMask{})
}

// Crop keeps the rectangle at x, y of width by height pixels.
func (e *Edit) Crop(x int, y int, width int, height int) *Edit {
	return e.Transform(PixelTransformCrop{X: x, Y: y, Width: width, Height: height})
}

// Zoom scales by numerator/denominator.
func (e *Edit) Zoom(numerator int, denominator int) *Edit {
	return e.Transform(PixelTransformZoom{Numerator: numerator, Denominator: denominator})
}

// Resize scales the frame to width by height pixels.
func (e *Edit) Resize(width int, height int) *Edit {
	return e.Transform(PixelTransformResize{Width: width, Height: height})
}

// Callout outlines the rectangle at x, y of width by height pixels, with an optional label.
func (e *Edit) Callout(x int, y int, width int, height int, label string) *Edit {
	callout := PixelTransformCallout{X: x, Y: y, Width: width, Height: height}
	if label != "" {
		callout.Label = new(label)
	}

	return e.Transform(callout)
}

// Transform adds a pixel transform.
func (e *Edit) Transform(transform PixelTransform) *Edit {
	e.plan.Transforms = append(e.plan.Transforms, transform)

	return e
}

// Plan returns the edit plan.
func (e *Edit) Plan() EditPlan {
	return e.plan
}

// Source returns the edit as the source the video calls take.
func (e *Edit) Source() EditSource {
	return EditSourcePlan{Edit: e.plan, ResourceBase: e.resourceBase}
}

// EditFile returns an edit plan file as the source the video calls take.
func EditFile(path string) EditSource {
	return EditSourceFile{Path: Path(path)}
}

// NormalizeEdit validates edit, resolves its subtitle paths, and returns it with its hash.
func (c *Client) NormalizeEdit(ctx context.Context, edit EditSource) (NormalizedEdit, error) {
	return c.editNormalize(ctx, normalizeParams{Edit: edit})
}

// VideoPlan resolves the presentation plan of edit over the completed session at session
// without rendering.
func (c *Client) VideoPlan(
	ctx context.Context,
	session string,
	edit EditSource,
	options VideoOptions,
) (PresentationPlan, error) {
	return c.videoPlan(ctx, planParams{Session: Path(session), Edit: edit, Options: options})
}

// VideoPreview renders a contact sheet and review video of edit into the new directory.
func (c *Client) VideoPreview(
	ctx context.Context,
	session string,
	edit EditSource,
	directory string,
	options VideoOptions,
) (VideoPreview, error) {
	return c.videoPreview(ctx, previewParams{
		Session:   Path(session),
		Edit:      edit,
		Directory: Path(directory),
		Options:   options,
	})
}

// VideoRender renders edit to out, whose .mp4, .gif, or .webm extension selects the format,
// and keeps a .termtrace sidecar beside it.
func (c *Client) VideoRender(
	ctx context.Context,
	session string,
	edit EditSource,
	out string,
	options VideoOptions,
) (VideoRender, error) {
	return c.videoRender(ctx, renderParams{
		Session: Path(session),
		Edit:    edit,
		Out:     Path(out),
		Options: options,
	})
}

// SourceForOutputFrame reports the clip and source frame that outputFrame shows, or nil past
// the end.
func (c *Client) SourceForOutputFrame(
	ctx context.Context,
	plan PresentationPlan,
	outputFrame uint64,
) (*OutputFrameSource, error) {
	return c.planSourceForOutputFrame(ctx, outputFrameParams{Plan: plan, OutputFrame: outputFrame})
}

// OutputFramesForSourceFrame lists the half-open output-frame spans that show sourceFrame.
func (c *Client) OutputFramesForSourceFrame(
	ctx context.Context,
	plan PresentationPlan,
	sourceFrame uint64,
) ([]Pair[uint64, uint64], error) {
	return c.planOutputFramesForSourceFrame(
		ctx,
		sourceFrameParams{Plan: plan, SourceFrame: sourceFrame},
	)
}

// NativeScreenshot replays the completed session at session in Ghostty and captures one state
// as a PNG in the new directory destination.
func (c *Client) NativeScreenshot(
	ctx context.Context,
	session string,
	destination string,
	options NativeScreenshotOptions,
) (CaptureOutcome, error) {
	return c.nativeScreenshot(ctx, screenshotParams{
		Session:     Path(session),
		Destination: Path(destination),
		Point:       options.Point,
		TimeoutMs:   milliseconds(options.Timeout),
	})
}

// PreflightVisualProfile validates profile and reports what a launch with it would use on a
// cols by rows grid; zero uses the default size.
func (c *Client) PreflightVisualProfile(
	ctx context.Context,
	profile VisualProfile,
	cols int,
	rows int,
) (VisualProfileReport, error) {
	return c.visualPreflight(ctx, preflightParams{
		Profile: profile,
		Cols:    positive(cols),
		Rows:    positive(rows),
	})
}

// RunScenarios runs scenario documents as one suite. Case failures stay in the report.
func (c *Client) RunScenarios(
	ctx context.Context,
	scenarios []string,
	options ScenarioSuiteOptions,
) (ScenarioSuiteReport, error) {
	paths := make([]Path, len(scenarios))
	for index, scenario := range scenarios {
		paths[index] = Path(scenario)
	}

	params := runScenariosParams{
		Scenarios:       paths,
		FailureRoot:     Path(options.FailureRoot),
		Jobs:            positive(options.Jobs),
		UpdateSnapshots: options.UpdateSnapshots,
		Reruns:          int(options.Reruns),
		FailOnFlaky:     options.FailOnFlaky,
	}
	if options.EvidenceRoot != "" {
		params.EvidenceRoot = new(Path(options.EvidenceRoot))
	}

	if options.Shard != nil {
		params.ShardIndex = new(options.Shard.Index)
		params.ShardCount = new(options.Shard.Count)
	}

	return c.scenarioRun(ctx, params)
}

// WriteScenarioJUnit writes report as JUnit XML to the new file path.
func (c *Client) WriteScenarioJUnit(
	ctx context.Context,
	path string,
	report ScenarioSuiteReport,
) error {
	return c.scenarioWriteJunit(ctx, junitParams{Path: Path(path), Report: report})
}

// ScenarioCIFailed reports whether CI must fail for report.
func (c *Client) ScenarioCIFailed(ctx context.Context, report ScenarioSuiteReport) (bool, error) {
	return c.scenarioCiFailed(ctx, ciParams{Report: report})
}
