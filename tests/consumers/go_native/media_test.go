package consumer

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"image/png"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

type visualProfileVector struct {
	Name        string                  `json:"name"`
	Native      bool                    `json:"native"`
	Profile     termtrace.VisualProfile `json:"profile"`
	Normalized  termtrace.VisualProfile `json:"normalized"`
	ProfileHash string                  `json:"profile_hash"`
}

type malformedProfileVector struct {
	Name    string         `json:"name"`
	Profile jsontext.Value `json:"profile"`
	Message string         `json:"message"`
}

type visualProfiles struct {
	Positive  []visualProfileVector    `json:"positive"`
	Invalid   []visualProfileVector    `json:"invalid"`
	Malformed []malformedProfileVector `json:"malformed"`
}

type editFacts struct {
	Field     string  `json:"field"`
	ClipIndex *uint64 `json:"clip_index"`
	Marker    *string `json:"marker"`
}

type negativeEdit struct {
	Name  string            `json:"name"`
	Edit  jsontext.Value    `json:"edit"`
	Files map[string]string `json:"files"`
	Code  string            `json:"code"`
	Facts editFacts         `json:"facts"`
}

type speedCase struct {
	Speed             [2]int `json:"speed"`
	RetimedFrameCount uint64 `json:"retimed_frame_count"`
}

type editing struct {
	Session struct {
		Grid struct {
			Cols int `json:"cols"`
			Rows int `json:"rows"`
		} `json:"grid"`
	} `json:"session"`
	FPS          int            `json:"fps"`
	Edit         jsontext.Value `json:"edit"`
	EditPlanHash string         `json:"edit_plan_hash"`
	Speeds       struct {
		FromNs       uint64      `json:"from_ns"`
		ToNs         uint64      `json:"to_ns"`
		TailToNs     uint64      `json:"tail_to_ns"`
		SourceFrames uint64      `json:"source_frames"`
		Cases        []speedCase `json:"cases"`
		Invalid      [][2]int    `json:"invalid"`
	} `json:"speeds"`
	Negative            []negativeEdit `json:"negative"`
	RelativeWithoutBase struct {
		Code  string    `json:"code"`
		Facts editFacts `json:"facts"`
	} `json:"relative_without_base"`
}

const captionsFile = "editing-captions.ass"

func semanticProfile() termtrace.VisualProfile {
	return termtrace.VisualProfile{
		Version:  1,
		Semantic: &termtrace.SemanticVisualProfile{Preset: termtrace.SemanticPresetBundledV1},
	}
}

func visualProfileFixture(t *testing.T) visualProfiles {
	t.Helper()
	return fixture[visualProfiles](t, "conformance/sdk/visual-profiles.json")
}

func editingFixture(t *testing.T) editing {
	t.Helper()
	return fixture[editing](t, "conformance/sdk/editing.json")
}

func sdkFixtures(t *testing.T) string {
	t.Helper()

	directory, err := filepath.EvalSymlinks(
		filepath.Join(repo(t), "fixtures", "conformance", "sdk"),
	)
	fatal(t, err)

	return directory
}

func readJSON(t *testing.T, path string) map[string]any {
	t.Helper()

	data, err := os.ReadFile(path)
	fatal(t, err)

	value := map[string]any{}
	fatal(t, json.Unmarshal(data, &value))

	return value
}

func copyFile(t *testing.T, source string, destination string) {
	t.Helper()

	data, err := os.ReadFile(source)
	fatal(t, err)
	fatal(t, os.WriteFile(destination, data, 0o644))
}

func wantCode(t *testing.T, err error, kind string, code string, name string) {
	t.Helper()

	if err == nil {
		t.Fatalf("%s: succeeded, want %s", name, code)
	}

	failure := termtraceError(t, err)
	if failure.Facts == nil || failure.Facts.Kind != kind || failure.Facts.Code != code {
		t.Fatalf("%s: error = %v, facts = %+v, want %s/%s", name, err, failure.Facts, kind, code)
	}
}

func wantEditFailure(t *testing.T, err error, code string, want editFacts, name string) {
	t.Helper()

	wantCode(t, err, "invalid_operation", code, name)

	failure := termtraceError(t, err)
	if failure.Facts.Edit == nil {
		t.Fatalf("%s: no edit facts in %v", name, err)
	}

	got := editFacts{
		Field:     failure.Facts.Edit.Field,
		ClipIndex: failure.Facts.Edit.ClipIndex,
		Marker:    failure.Facts.Edit.Marker,
	}
	if !sameJSON(t, got, want) {
		t.Errorf("%s: edit facts = %+v, want %+v", name, got, want)
	}
}

func TestVisualProfileVectorsAgreeWithTheSharedContract(t *testing.T) {
	vectors := visualProfileFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 120*time.Second)

	for _, vector := range vectors.Positive {
		report, err := client.PreflightVisualProfile(ctx, vector.Profile, 40, 10)
		fatal(t, err)

		if report.Version != 1 || report.ProfileHash != vector.ProfileHash {
			t.Errorf("%s: version %d hash %s", vector.Name, report.Version, report.ProfileHash)
		}

		if !sameJSON(t, report.Profile, vector.Normalized) {
			t.Errorf("%s: profile = %+v", vector.Name, report.Profile)
		}

		if vector.Native {
			native := report.Native
			if native == nil {
				t.Fatalf("%s: no native report", vector.Name)
			}

			if native.DPI != 96 ||
				!sameJSON(t, native.DiscoveredFontFamilies, []string{"JetBrains Mono"}) ||
				native.Geometry.RequiredCols != 40 || native.Geometry.RequiredRows != 10 {
				t.Errorf("%s: native = %+v", vector.Name, native)
			}
		} else if report.Native != nil {
			t.Errorf("%s: native = %+v, want none", vector.Name, report.Native)
		}

		if vector.Normalized.Semantic != nil {
			if report.Semantic == nil || report.Semantic.CellWidth != 12 ||
				report.Semantic.CellHeight != 24 {
				t.Errorf("%s: semantic = %+v", vector.Name, report.Semantic)
			}
		} else if report.Semantic != nil {
			t.Errorf("%s: semantic = %+v, want none", vector.Name, report.Semantic)
		}
	}

	for _, vector := range vectors.Invalid {
		_, err := client.PreflightVisualProfile(ctx, vector.Profile, 0, 0)
		wantCode(t, err, "invalid_operation", "invalid_visual_profile", vector.Name)
	}

	for _, vector := range vectors.Malformed {
		var profile termtrace.VisualProfile

		switch vector.Name {
		case "numeric-size":
			if err := termtrace.DecodeJSON(vector.Profile, &profile); err == nil {
				t.Errorf("%s: decoded %+v", vector.Name, profile)
			}
		case "unsupported-preset":
			fatal(t, termtrace.DecodeJSON(vector.Profile, &profile))

			_, err := client.PreflightVisualProfile(ctx, profile, 0, 0)
			if err == nil || !strings.Contains(err.Error(), vector.Message) {
				t.Errorf("%s: error = %v, want %q", vector.Name, err, vector.Message)
			}
		}
	}
}

func TestASemanticVisualProfileRecordsWithoutNativeTools(t *testing.T) {
	t.Setenv("PATH", "")

	vectors := visualProfileFixture(t)
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)

	report, err := client.PreflightVisualProfile(ctx, semanticProfile(), 0, 0)
	fatal(t, err)

	if report.Native != nil {
		t.Errorf("native = %+v", report.Native)
	}

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo SEMANTIC_PROFILE"},
		termtrace.LaunchOptions{VisualProfile: new(semanticProfile())},
	)
	_, err = recording.Output().WaitFor(ctx, []byte("SEMANTIC_PROFILE"), 5*time.Second)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	manifest := readJSON(t, filepath.Join(string(recording.Path()), "session.json"))
	visual, _ := manifest["visual_profile"].(map[string]any)

	if manifest["version"] != 13.0 || visual["profile_hash"] != report.ProfileHash {
		t.Errorf("manifest version %v visual profile %v", manifest["version"], visual)
	}

	if ghostty, present := manifest["ghostty_visual"]; !present || ghostty != nil {
		t.Errorf("ghostty_visual = %v (present %v)", ghostty, present)
	}

	_, err = client.PreflightVisualProfile(ctx, vectors.Positive[1].Profile, 0, 0)
	if failure := termtraceError(t, err); failure.Facts == nil ||
		failure.Facts.Code != "native_profile_unavailable" {
		t.Errorf("native preflight = %v", err)
	}
}

func TestNativeVisualProfileFailuresStopBeforeTheApplicationStarts(t *testing.T) {
	vectors := visualProfileFixture(t)
	client, dir := newClient(t)
	ctx := within(t, 60*time.Second)
	marker := filepath.Join(dir, "started")
	command := []string{"/bin/sh", "-c", `printf started > "$1"`, "probe", marker}

	_, err := client.Launch(ctx, command, termtrace.LaunchOptions{
		VisualProfile: &termtrace.VisualProfile{
			Version: 1,
			Native: &termtrace.NativeVisualProfile{
				FontFamily: new("Termtrace Missing Family 7c1d"),
			},
		},
	})
	wantCode(t, err, "engine", "font_unavailable", "missing font")

	_, err = client.Launch(ctx, command, termtrace.LaunchOptions{
		VisualProfile: new(vectors.Invalid[2].Profile),
	})
	if failure := termtraceError(t, err); failure.Facts == nil ||
		failure.Facts.Code != "invalid_visual_profile" {
		t.Errorf("invalid profile launch = %v", err)
	}

	if _, err := os.Stat(marker); err == nil {
		t.Error("the application started")
	}

	entries, err := os.ReadDir(filepath.Join(dir, "sessions"))
	if err == nil && len(entries) > 0 {
		t.Errorf("evidence root holds %d entries", len(entries))
	}
}

func fixtureEdit(base string, subtitle string) *termtrace.Edit {
	defaultStyle := termtrace.AnnotationStyle{
		FontSize:          20,
		Foreground:        "#ffffff",
		Background:        "#000000",
		BackgroundOpacity: 160,
		Border:            1,
	}
	edit := termtrace.NewEdit()

	if base != "" {
		edit.WithResourceBase(base)
	}

	return edit.
		Clip(
			termtrace.SelectMarker("intro"),
			termtrace.SelectMarker("middle"),
			termtrace.ClipOptions{
				Speed:   termtrace.RationalSpeed{Numerator: 2, Denominator: 3},
				Hold:    500 * time.Millisecond,
				Caption: "First part",
			},
		).
		Clip(
			termtrace.SelectMarker("middle"),
			termtrace.SelectMarker("outro"),
			termtrace.ClipOptions{Speed: termtrace.RationalSpeed{Numerator: 3, Denominator: 2}},
		).
		Annotate(termtrace.TextAnnotation{
			Text:  new("Example"),
			Scope: termtrace.AnnotationScopeWholeVideo{},
		}).
		Annotate(termtrace.TextAnnotation{
			Template: new("frame {output_frame}"),
			Scope:    termtrace.AnnotationScopeClip{ClipIndex: 1},
			Position: termtrace.AnnotationPosition{
				Anchor: termtrace.AnnotationAnchorBottomRight,
				X:      8,
				Y:      8,
			},
			Style: termtrace.AnnotationStyle{
				FontSize:          16,
				Foreground:        "#ffff00",
				Background:        "#202020",
				BackgroundOpacity: 200,
				Border:            0,
			},
			ZOrder: 1,
		}).
		Annotate(termtrace.TextAnnotation{
			Text: new("Range"),
			Scope: termtrace.AnnotationScopeRange{
				From: termtrace.SelectMarker("intro"),
				To:   termtrace.SelectMarker("middle"),
			},
			Position: termtrace.AnnotationPosition{Anchor: termtrace.AnnotationAnchorCenter},
			Style:    defaultStyle,
			ZOrder:   2,
		}).
		Annotate(termtrace.TextAnnotation{
			Text:  new("Start"),
			Scope: termtrace.AnnotationScopeOutputFrame{OutputFrame: 0},
			Position: termtrace.AnnotationPosition{
				Anchor: termtrace.AnnotationAnchorTopRight,
				X:      16,
				Y:      16,
			},
			ZOrder: 3,
		}).
		Subtitles(subtitle).
		Changes(termtrace.ChangeVisualization{
			Source:     termtrace.ChangeSourceSemanticCells,
			Mode:       termtrace.ChangeModeBoundingBox,
			HoldFrames: 2,
		}).
		CaptionLayout(termtrace.CaptionLayoutFooter).
		Branding("Example").
		Delivery(termtrace.PresentationDelivery{
			Codec:         "h264",
			PixelFormat:   "yuv420p",
			Crf:           new(20),
			EncoderPreset: new("slow"),
		}).
		CursorMask().
		Crop(0, 0, 480, 240).
		Zoom(2, 1).
		Callout(16, 16, 200, 100, "note")
}

// recordEditSession records a semantic session with the markers the editing fixture selects.
func recordEditSession(t *testing.T, fixture editing) (*termtrace.Client, string) {
	t.Helper()

	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(
		t,
		client,
		[]string{
			"/bin/sh",
			"-c",
			"trap 'exit 0' TERM; i=0; while :; do i=$((i+1)); printf 'line %s\\n' $i; sleep 0.4; done",
		},
		termtrace.LaunchOptions{
			Rows:          new(fixture.Session.Grid.Rows),
			Cols:          new(fixture.Session.Grid.Cols),
			VisualProfile: new(semanticProfile()),
		},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("line 1"), 5*time.Second)
	fatal(t, err)

	for _, step := range []struct {
		delay time.Duration
		label string
	}{
		{300 * time.Millisecond, "intro"},
		{600 * time.Millisecond, "middle"},
		{100 * time.Millisecond, "repeat"},
		{100 * time.Millisecond, "repeat"},
		{500 * time.Millisecond, "outro"},
	} {
		time.Sleep(step.delay)

		_, err := recording.Mark(ctx, step.label, termtrace.MutationOptions{})
		fatal(t, err)
	}

	time.Sleep(300 * time.Millisecond)
	fatal(t, recording.Stop(ctx))

	return client, string(recording.Path())
}

func semanticOptions(fixture editing) termtrace.VideoOptions {
	return termtrace.VideoOptions{Renderer: termtrace.VideoRendererSemantic, FPS: new(fixture.FPS)}
}

func writeEditFile(
	t *testing.T,
	directory string,
	edit jsontext.Value,
	files map[string]string,
) string {
	t.Helper()
	fatal(t, os.MkdirAll(directory, 0o755))

	for name, contents := range files {
		fatal(t, os.WriteFile(filepath.Join(directory, name), []byte(contents), 0o644))
	}

	file := filepath.Join(directory, "edit.json")
	fatal(t, os.WriteFile(file, edit, 0o644))

	return file
}

func withoutSubtitlePaths(plan termtrace.PresentationPlan) termtrace.PresentationPlan {
	subtitles := make([]termtrace.NormalizedSubtitle, len(plan.Subtitles))
	for index, subtitle := range plan.Subtitles {
		subtitle.SourcePath = ""
		subtitles[index] = subtitle
	}

	plan.Subtitles = subtitles

	return plan
}

func TestFixtureEditMatchesTheRustNormalizationAndFilePlan(t *testing.T) {
	fixture := editingFixture(t)
	fixtures := sdkFixtures(t)
	client, session := recordEditSession(t, fixture)
	ctx := within(t, 120*time.Second)
	edit := fixtureEdit(fixtures, captionsFile)

	normalized, err := client.NormalizeEdit(ctx, edit.Source())
	fatal(t, err)

	if normalized.EditPlanHash != fixture.EditPlanHash {
		t.Errorf("normalized hash = %s", normalized.EditPlanHash)
	}

	copied := filepath.Join(t.TempDir(), "fixture-file")
	fatal(t, os.Mkdir(copied, 0o755))
	copyFile(t, filepath.Join(fixtures, captionsFile), filepath.Join(copied, captionsFile))

	fromFile, err := client.VideoPlan(
		ctx,
		session,
		termtrace.EditFile(writeEditFile(t, copied, fixture.Edit, nil)),
		semanticOptions(fixture),
	)
	fatal(t, err)
	fromBuilder, err := client.VideoPlan(ctx, session, edit.Source(), semanticOptions(fixture))
	fatal(t, err)

	if fromBuilder.EditPlanHash != fixture.EditPlanHash {
		t.Errorf("builder plan hash = %s", fromBuilder.EditPlanHash)
	}

	if !sameJSON(t, withoutSubtitlePaths(fromBuilder), withoutSubtitlePaths(fromFile)) {
		t.Error("the builder plan differs from the file plan")
	}

	for frame := range fromBuilder.OutputFrameCount {
		found, err := client.SourceForOutputFrame(ctx, fromBuilder, frame)
		fatal(t, err)

		if found == nil {
			t.Fatalf("frame %d has no source", frame)
		}

		spans, err := client.OutputFramesForSourceFrame(ctx, fromBuilder, found.SourceFrame)
		fatal(t, err)

		covered := false

		for _, span := range spans {
			if span.First <= frame && frame < span.Second {
				covered = true
			}
		}

		if !covered {
			t.Errorf("frame %d is outside the spans %v of source frame %d",
				frame, spans, found.SourceFrame)
		}
	}

	past, err := client.SourceForOutputFrame(ctx, fromBuilder, fromBuilder.OutputFrameCount)
	fatal(t, err)

	if past != nil {
		t.Errorf("past the end = %+v", past)
	}

	defaults, err := client.NormalizeEdit(ctx, termtrace.NewEdit().Clip(
		termtrace.SelectMonotonicNs(0),
		termtrace.SelectMonotonicNs(1),
		termtrace.ClipOptions{},
	).Source())
	fatal(t, err)

	layout := defaults.Edit.Layout
	if layout.Caption != termtrace.CaptionLayoutFooter || layout.Branding != nil {
		t.Errorf("default layout = %+v", layout)
	}

	if speed := defaults.Edit.Clips[0].Speed; speed.Numerator != 1 || speed.Denominator != 1 {
		t.Errorf("default speed = %+v", speed)
	}
}

func TestRelativeSubtitlesNeedAnExplicitResourceBase(t *testing.T) {
	fixture := editingFixture(t)
	fixtures := sdkFixtures(t)
	client, dir := newClient(t)
	ctx := within(t, 30*time.Second)

	_, err := client.NormalizeEdit(ctx, fixtureEdit("", captionsFile).Source())
	wantEditFailure(
		t,
		err,
		fixture.RelativeWithoutBase.Code,
		fixture.RelativeWithoutBase.Facts,
		"relative",
	)

	absolute, err := client.NormalizeEdit(
		ctx,
		fixtureEdit("", filepath.Join(fixtures, captionsFile)).Source(),
	)
	fatal(t, err)

	if absolute.EditPlanHash != fixture.EditPlanHash {
		t.Errorf("absolute hash = %s", absolute.EditPlanHash)
	}

	elsewhere := filepath.Join(dir, "elsewhere")
	file := writeEditFile(t, elsewhere, fixture.Edit, nil)
	copyFile(t, filepath.Join(fixtures, captionsFile), filepath.Join(elsewhere, captionsFile))

	fromFile, err := client.NormalizeEdit(ctx, termtrace.EditFile(file))
	fatal(t, err)

	if fromFile.EditPlanHash != fixture.EditPlanHash {
		t.Errorf("file hash = %s", fromFile.EditPlanHash)
	}
}

func TestEditFilesAcceptNonUTF8Paths(t *testing.T) {
	fixture := editingFixture(t)
	client, dir := newClient(t)
	subtitle, err := json.Marshal(filepath.Join(sdkFixtures(t), captionsFile))
	fatal(t, err)

	relative := []byte(`"` + captionsFile + `"`)
	if !bytes.Contains(fixture.Edit, relative) {
		t.Fatalf("the fixture edit does not name %s", captionsFile)
	}

	edit := bytes.Replace(fixture.Edit, relative, subtitle, 1)
	file := writeEditFile(t, filepath.Join(dir, "bytes-"+string([]byte{0xff})), edit, nil)

	normalized, err := client.NormalizeEdit(within(t, 30*time.Second), termtrace.EditFile(file))
	fatal(t, err)

	if normalized.EditPlanHash != fixture.EditPlanHash {
		t.Errorf("hash = %s", normalized.EditPlanHash)
	}
}

func TestSelectorsKeepExact64BitValues(t *testing.T) {
	client, _ := newClient(t)
	wide := uint64(1)<<63 + 1
	maximum := ^uint64(0)

	normalized, err := client.NormalizeEdit(within(t, 30*time.Second), termtrace.NewEdit().Clip(
		termtrace.SelectEvent(wide),
		termtrace.SelectState(maximum),
		termtrace.ClipOptions{},
	).Source())
	fatal(t, err)

	clip := normalized.Edit.Clips[0]
	if clip.From.EventSeq == nil || *clip.From.EventSeq != wide ||
		clip.To.StateId == nil || *clip.To.StateId != maximum {
		t.Errorf("clip = %+v", clip)
	}
}

func TestSpeedsProduceTheDocumentedFrameCounts(t *testing.T) {
	fixture := editingFixture(t)
	speeds := fixture.Speeds
	client, session := recordEditSession(t, fixture)
	ctx := within(t, 60*time.Second)
	edit := func(speed [2]int) termtrace.EditSource {
		return termtrace.NewEdit().
			Clip(
				termtrace.SelectMonotonicNs(speeds.FromNs),
				termtrace.SelectMonotonicNs(speeds.ToNs),
				termtrace.ClipOptions{
					Speed: termtrace.RationalSpeed{Numerator: speed[0], Denominator: speed[1]},
				},
			).
			Clip(
				termtrace.SelectMonotonicNs(speeds.ToNs),
				termtrace.SelectMonotonicNs(speeds.TailToNs),
				termtrace.ClipOptions{},
			).
			Source()
	}

	for _, speedCase := range speeds.Cases {
		plan, err := client.VideoPlan(ctx, session, edit(speedCase.Speed), semanticOptions(fixture))
		fatal(t, err)

		clip := plan.Clips[0]
		if clip.SourceEndFrame-clip.SourceStartFrame != speeds.SourceFrames ||
			clip.RetimedFrameCount != speedCase.RetimedFrameCount {
			t.Errorf("speed %v: clip = %+v", speedCase.Speed, clip)
		}
	}

	for _, speed := range speeds.Invalid {
		_, err := client.NormalizeEdit(ctx, edit(speed))
		wantEditFailure(
			t,
			err,
			"invalid_speed",
			editFacts{Field: "clips[0].speed", ClipIndex: new(uint64(0)), Marker: nil},
			"speed",
		)
	}
}

func TestSharedNegativeEditsFailWithTheSameCodesAndFacts(t *testing.T) {
	fixture := editingFixture(t)
	client, session := recordEditSession(t, fixture)
	ctx := within(t, 60*time.Second)
	root := t.TempDir()

	for _, negative := range fixture.Negative {
		file := writeEditFile(t, filepath.Join(root, negative.Name), negative.Edit, negative.Files)
		_, err := client.VideoPlan(ctx, session, termtrace.EditFile(file), semanticOptions(fixture))
		wantEditFailure(t, err, negative.Code, negative.Facts, negative.Name)
	}
}

func probeVideo(t *testing.T, path string, entries string, extra ...string) string {
	t.Helper()

	arguments := append([]string{"-v", "error"}, extra...)
	arguments = append(arguments, "-select_streams", "v:0", "-show_entries", entries)
	arguments = append(arguments, "-of", "json", path)

	output, err := exec.CommandContext(t.Context(), "ffprobe", arguments...).Output()
	fatal(t, err)

	return string(output)
}

type probedStreams struct {
	Streams []struct {
		CodecName    string `json:"codec_name"`
		NbReadFrames string `json:"nb_read_frames"`
	} `json:"streams"`
}

func probedStream(t *testing.T, path string, entries string, extra ...string) (string, string) {
	t.Helper()

	var probed probedStreams
	fatal(t, json.Unmarshal([]byte(probeVideo(t, path, entries, extra...)), &probed))

	if len(probed.Streams) != 1 {
		t.Fatalf("%s: streams = %+v", path, probed.Streams)
	}

	return probed.Streams[0].CodecName, probed.Streams[0].NbReadFrames
}

func presentationSourceKind(t *testing.T, artifact *termtrace.MediaArtifactResult) string {
	t.Helper()

	if artifact == nil {
		t.Fatal("no artifact")
	}

	output, ok := artifact.Provenance.(termtrace.ArtifactProvenancePresentationOutput)
	if !ok {
		t.Fatalf("provenance = %T", artifact.Provenance)
	}

	return string(output.SourceKind)
}

func TestSemanticPreviewAndEveryFormatRender(t *testing.T) {
	fixture := editingFixture(t)
	client, session := recordEditSession(t, fixture)
	ctx := within(t, 300*time.Second)
	edit := fixtureEdit(sdkFixtures(t), captionsFile).Source()
	output := filepath.Join(t.TempDir(), "renders")
	fatal(t, os.Mkdir(output, 0o755))

	preview, err := client.VideoPreview(
		ctx,
		session,
		edit,
		filepath.Join(output, "preview"),
		semanticOptions(fixture),
	)
	fatal(t, err)

	if info, err := os.Stat(string(preview.ContactSheet)); err != nil || !info.Mode().IsRegular() {
		t.Errorf("contact sheet %s: %v", preview.ContactSheet, err)
	}

	if preview.Plan.EditPlanHash != fixture.EditPlanHash {
		t.Errorf("preview hash = %s", preview.Plan.EditPlanHash)
	}

	completion := readJSON(t, filepath.Join(session, "completion.json"))

	for _, format := range []struct{ extension, codec string }{
		{"mp4", "h264"},
		{"gif", "gif"},
		{"webm", "vp9"},
	} {
		out := filepath.Join(output, "edited."+format.extension)
		render, err := client.VideoRender(ctx, session, edit, out, semanticOptions(fixture))
		fatal(t, err)

		if string(render.Output) != out {
			t.Errorf("%s: output = %s", format.extension, render.Output)
		}

		if codec, _ := probedStream(t, out, "stream=codec_name"); codec != format.codec {
			t.Errorf("%s: codec = %s", format.extension, codec)
		}

		manifest := readJSON(t, filepath.Join(string(render.Sidecar), "manifest.json"))

		if format.extension == "mp4" {
			if render.Artifact != nil || manifest["source_truth_layer"] != "semantic_rendering" {
				t.Errorf("mp4: artifact %+v, truth layer %v",
					render.Artifact, manifest["source_truth_layer"])
			}

			continue
		}

		if kind := presentationSourceKind(t, render.Artifact); kind != "semantic_rendering" {
			t.Errorf("%s: source kind = %s", format.extension, kind)
		}

		if render.Artifact.Source.SessionManifestHash != completion["session_manifest_hash"] {
			t.Errorf("%s: manifest hash = %s", format.extension,
				render.Artifact.Source.SessionManifestHash)
		}
	}

	existing := filepath.Join(output, "edited.mp4")
	before, err := os.ReadFile(existing)
	fatal(t, err)

	_, err = client.VideoRender(ctx, session, edit, existing, semanticOptions(fixture))
	if err == nil || !strings.Contains(err.Error(), "already exists") {
		t.Errorf("render over an existing file = %v", err)
	}

	after, err := os.ReadFile(existing)
	fatal(t, err)

	if string(after) != string(before) {
		t.Error("the existing file changed")
	}
}

func capabilityMap(t *testing.T, client *termtrace.Client) map[string]termtrace.Capability {
	t.Helper()

	report, err := client.Capabilities(within(t, 60*time.Second))
	fatal(t, err)

	result := map[string]termtrace.Capability{}
	for _, capability := range report.Capabilities {
		result[capability.ID] = capability
	}

	return result
}

func ffmpegWithoutVP9(t *testing.T, directory string) string {
	t.Helper()

	ffmpeg, err := exec.LookPath("ffmpeg")
	fatal(t, err)
	fatal(t, os.Mkdir(directory, 0o755))

	script := "#!/bin/sh\ncase \" $* \" in *libvpx-vp9*) echo 'Unknown encoder libvpx-vp9' >&2; exit 1;; esac\nexec " +
		quote(
			ffmpeg,
		) + " \"$@\"\n"
	fatal(t, os.WriteFile(filepath.Join(directory, "ffmpeg"), []byte(script), 0o700))

	return directory
}

func TestDeliveryCodecAvailabilityFollowsTheFFmpegOnPath(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 120*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf READY; IFS= read -r line"},
		termtrace.LaunchOptions{Cols: new(30), Rows: new(6)},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("READY"), 5*time.Second)
	fatal(t, err)

	for _, label := range []string{"start", "end"} {
		_, err := recording.Mark(ctx, label, termtrace.MutationOptions{})
		fatal(t, err)
	}

	fatal(t, recording.Stop(ctx))

	session := string(recording.Path())
	edit := termtrace.NewEdit().
		Clip(termtrace.SelectMarker("start"), termtrace.SelectMarker("end"), termtrace.ClipOptions{}).
		Source()
	semantic := termtrace.VideoOptions{Renderer: termtrace.VideoRendererSemantic}

	host := capabilityMap(t, client)
	if host["gif-delivery"].Status != termtrace.CapabilityStatusReady ||
		host["webm-delivery"].Status != termtrace.CapabilityStatusReady {
		t.Errorf("host delivery = %+v, %+v", host["gif-delivery"], host["webm-delivery"])
	}

	t.Setenv(
		"PATH",
		ffmpegWithoutVP9(t, filepath.Join(root, "tools"))+string(os.PathListSeparator)+
			os.Getenv("PATH"),
	)

	limitedClient, _ := newClient(t)
	limited := capabilityMap(t, limitedClient)
	webm := limited["webm-delivery"]

	if limited["gif-delivery"].Status != termtrace.CapabilityStatusReady ||
		webm.Status != termtrace.CapabilityStatusBlocked ||
		webm.Code != "derivative_encoder_unavailable" ||
		webm.Remediation == nil || *webm.Remediation == "" {
		t.Errorf("limited delivery = %+v, %+v", limited["gif-delivery"], webm)
	}

	out := filepath.Join(root, "blocked.webm")

	_, err = limitedClient.VideoRender(ctx, session, edit, out, semantic)
	if err == nil ||
		!regexp.MustCompile(`(?s)webm-delivery.*derivative_encoder_unavailable`).
			MatchString(err.Error()) {
		t.Errorf("blocked render = %v", err)
	}

	if _, err := os.Stat(out); err == nil {
		t.Error("the blocked render wrote its output")
	}

	gif, err := limitedClient.VideoRender(
		ctx,
		session,
		edit,
		filepath.Join(root, "allowed.gif"),
		semantic,
	)
	fatal(t, err)

	if info, err := os.Stat(string(gif.Output)); err != nil || !info.Mode().IsRegular() {
		t.Errorf("gif %s: %v", gif.Output, err)
	}
}

type replaySession struct {
	client  *termtrace.Client
	root    string
	session string
	first   string
	second  string
}

// recordReplaySession records FIRST and SECOND on a 30 by 6 grid with markers around SECOND.
func recordReplaySession(t *testing.T) replaySession {
	t.Helper()

	client, root := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf FIRST; sleep 0.5; printf ' SECOND'; IFS= read -r line"},
		termtrace.LaunchOptions{Cols: new(30), Rows: new(6)},
	)
	first, err := recording.Screen().WaitForText(ctx, "FIRST", 5*time.Second)
	fatal(t, err)
	_, err = recording.Mark(ctx, "start", termtrace.MutationOptions{})
	fatal(t, err)
	second, err := recording.Screen().WaitForText(ctx, "SECOND", 5*time.Second)
	fatal(t, err)
	_, err = recording.Mark(ctx, "end", termtrace.MutationOptions{})
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	return replaySession{
		client:  client,
		root:    root,
		session: string(recording.Path()),
		first:   first.Hash,
		second:  second.Hash,
	}
}

func pngSize(t *testing.T, path string) (int, int) {
	t.Helper()

	file, err := os.Open(path)
	fatal(t, err)

	defer func() { _ = file.Close() }()

	config, err := png.DecodeConfig(file)
	fatal(t, err)

	return config.Width, config.Height
}

func TestAScreenshotCapturesTheSelectedStateAsNativePixels(t *testing.T) {
	replay := recordReplaySession(t)
	ctx := within(t, 120*time.Second)

	selected, err := replay.client.NativeScreenshot(
		ctx,
		replay.session,
		filepath.Join(replay.root, "first"),
		termtrace.NativeScreenshotOptions{
			Point: termtrace.StatePointStateHash{StateHash: replay.first},
		},
	)
	fatal(t, err)
	latest, err := replay.client.NativeScreenshot(
		ctx,
		replay.session,
		filepath.Join(replay.root, "latest"),
		termtrace.NativeScreenshotOptions{},
	)
	fatal(t, err)

	for _, capture := range []struct {
		outcome termtrace.CaptureOutcome
		state   string
	}{{selected, replay.first}, {latest, replay.second}} {
		directory := string(capture.outcome.Directory)
		manifest := readJSON(t, filepath.Join(directory, "manifest.json"))
		geometry, _ := manifest["geometry_report"].(map[string]any)
		version, _ := manifest["ghostty_version"].(string)

		if capture.outcome.Video != nil ||
			manifest["capture_kind"] != "screenshot" ||
			manifest["guarantee_level"] != "native_capture_environment" ||
			!strings.HasPrefix(version, "Ghostty") ||
			manifest["selected_state_hash"] != capture.state ||
			geometry["cols"] != 30.0 || geometry["rows"] != 6.0 {
			t.Errorf("capture %s: manifest = %v", directory, manifest)
		}

		width, height := pngSize(t, string(capture.outcome.Screenshot))
		if float64(width) != manifest["width"] || float64(height) != manifest["height"] {
			t.Errorf(
				"png %dx%d, manifest %vx%v",
				width,
				height,
				manifest["width"],
				manifest["height"],
			)
		}

		members, _ := manifest["members"].([]any)
		image := ""

		for _, member := range members {
			entry, _ := member.(map[string]any)
			if entry["relationship"] == "native_final_frame" {
				image, _ = entry["path"].(string)
			}
		}

		if filepath.Join(directory, image) != string(capture.outcome.Screenshot) {
			t.Errorf("final frame member %q, screenshot %s", image, capture.outcome.Screenshot)
		}
	}

	first, err := os.ReadFile(string(selected.Screenshot))
	fatal(t, err)
	last, err := os.ReadFile(string(latest.Screenshot))
	fatal(t, err)

	if string(first) == string(last) {
		t.Error("the selected and latest screenshots are identical")
	}
}

func directoryContents(t *testing.T, directory string) map[string]string {
	t.Helper()

	entries, err := os.ReadDir(directory)
	fatal(t, err)

	contents := map[string]string{}

	for _, entry := range entries {
		data, err := os.ReadFile(filepath.Join(directory, entry.Name()))
		fatal(t, err)

		contents[entry.Name()] = string(data)
	}

	return contents
}

func TestAScreenshotNeverReplacesAnExistingDestination(t *testing.T) {
	replay := recordReplaySession(t)
	ctx := within(t, 120*time.Second)
	destination := filepath.Join(replay.root, "shot")

	_, err := replay.client.NativeScreenshot(
		ctx,
		replay.session,
		destination,
		termtrace.NativeScreenshotOptions{},
	)
	fatal(t, err)

	before := directoryContents(t, destination)

	_, err = replay.client.NativeScreenshot(
		ctx,
		replay.session,
		destination,
		termtrace.NativeScreenshotOptions{Point: termtrace.StatePointMarker{Label: "start"}},
	)
	if err == nil {
		t.Error("a second screenshot into the same destination succeeded")
	}

	if !maps.Equal(directoryContents(t, destination), before) {
		t.Error("the existing destination changed")
	}
}

func TestTheNativeRendererReplaysTheSessionIntoANativeVideo(t *testing.T) {
	replay := recordReplaySession(t)
	ctx := within(t, 300*time.Second)
	out := filepath.Join(replay.root, "native.webm")
	edit := termtrace.NewEdit().
		Clip(termtrace.SelectMarker("start"), termtrace.SelectMarker("end"), termtrace.ClipOptions{}).
		Source()

	render, err := replay.client.VideoRender(ctx, replay.session, edit, out, termtrace.VideoOptions{
		Renderer: termtrace.VideoRendererNative,
		FPS:      new(24),
	})
	fatal(t, err)

	if kind := presentationSourceKind(t, render.Artifact); kind != "native_replay" {
		t.Errorf("source kind = %s", kind)
	}

	source, ok := render.Plan.Source.(termtrace.PresentationSourceNative)
	if !ok {
		t.Fatalf("plan source = %T", render.Plan.Source)
	}

	codec, frames := probedStream(t, out, "stream=codec_name,nb_read_frames", "-count_frames")
	if count, err := strconv.Atoi(frames); codec != "vp9" || err != nil || count <= 0 {
		t.Errorf("codec %s, frames %s", codec, frames)
	}

	capture := readJSON(t, filepath.Join(filepath.Dir(source.Video), "manifest.json"))
	if capture["capture_kind"] != "video" ||
		capture["guarantee_level"] != "native_capture_environment" {
		t.Errorf("capture manifest = %v", capture)
	}
}
