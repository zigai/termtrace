package consumer

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"io/fs"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

type diagnosticsEvidence struct {
	SaveFormats []termtrace.SaveFormat    `json:"save_formats"`
	Profiles    []termtrace.BundleProfile `json:"profiles"`
	Selectors   []string                  `json:"selectors"`
	U128Max     string                    `json:"u128_max"`

	ApplicationAttributes map[string]any `json:"application_attributes"`
}

func diagnosticsFixture(t *testing.T) diagnosticsEvidence {
	t.Helper()
	return fixture[diagnosticsEvidence](t, "conformance/sdk/diagnostics-evidence.json")
}

func topLevelHashes(t *testing.T, root string) map[string]string {
	t.Helper()

	entries, err := os.ReadDir(root)
	fatal(t, err)

	result := map[string]string{}

	for _, entry := range entries {
		if !entry.Type().IsRegular() {
			continue
		}

		data, err := os.ReadFile(filepath.Join(root, entry.Name()))
		fatal(t, err)

		digest := sha256.Sum256(data)
		result[entry.Name()] = hex.EncodeToString(digest[:])
	}

	return result
}

func canonicalReplay(observation termtrace.Observation) termtrace.Observation {
	observation.Resolution = termtrace.ObservationResolutionCanonicalReplay
	return observation
}

type completedSession struct {
	client     *termtrace.Client
	root       string
	recording  *termtrace.Recording
	incomplete string
}

// completed records a session that marks "cut", emits application events and spans, reads one
// line, prints DONE, and stops. The incomplete span is left open.
func completed(t *testing.T) completedSession {
	t.Helper()

	client, root := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf READY; read line; printf DONE; read line"},
		termtrace.LaunchOptions{Cols: new(40), Rows: new(8)},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("READY"), 5*time.Second)
	fatal(t, err)
	_, err = recording.Mark(ctx, "cut", termtrace.MutationOptions{})
	fatal(t, err)

	incomplete := instrument(t, recording)

	_, err = recording.Actions().SendText("go\n").UntilScreen("DONE", 5*time.Second).Run(ctx)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	return completedSession{
		client:     client,
		root:       root,
		recording:  recording,
		incomplete: incomplete,
	}
}

// instrument emits the fixture event, a complete span, and an incomplete child span, checks that
// unsafe attributes are rejected, and returns the incomplete span.
func instrument(t *testing.T, recording *termtrace.Recording) string {
	t.Helper()

	ctx := within(t, 30*time.Second)
	emitter, err := recording.Instrumentation(ctx)
	fatal(t, err)

	t.Cleanup(func() {
		if err := emitter.Close(context.Background()); err != nil {
			t.Errorf("close emitter: %v", err)
		}
	})

	ack, err := emitter.Event(ctx, "fixture", diagnosticsFixture(t).ApplicationAttributes)
	fatal(t, err)

	if !ack.Acknowledged || ack.ProcessInstanceID != emitter.ProcessInstanceID() ||
		ack.Boundaries.Durable.NextEventSeq == 0 {
		t.Errorf("event acknowledgement = %+v", ack)
	}

	span, err := emitter.StartSpan(ctx, "complete", "", nil)
	fatal(t, err)

	if span.SpanID == nil {
		t.Fatalf("span start has no span id: %+v", span)
	}

	ended, err := emitter.EndSpan(ctx, "complete", *span.SpanID, nil)
	fatal(t, err)

	if ended.SpanID == nil || *ended.SpanID != *span.SpanID {
		t.Errorf("ended span = %v, want %s", ended.SpanID, *span.SpanID)
	}

	incomplete, err := emitter.StartSpan(ctx, "incomplete", *span.SpanID, nil)
	fatal(t, err)

	if incomplete.SpanID == nil {
		t.Fatalf("incomplete span has no span id: %+v", incomplete)
	}

	_, err = emitter.Event(
		ctx,
		"unsafe",
		map[string]any{"nested": map[string]any{"inode": int64(9007199254740992)}},
	)
	if err == nil || !strings.Contains(err.Error(), "safe-integer") {
		t.Errorf("event with an unsafe integer = %v", err)
	}

	var deep any
	for range 130 {
		deep = []any{deep}
	}

	for name, invalid := range map[string]map[string]any{
		"deep":  {"deep": deep},
		"large": {"large": strings.Repeat("x", 65536)},
		"many":  {"many": make([]any, 65537)},
	} {
		if _, err := emitter.Event(ctx, "unsafe", invalid); err == nil {
			t.Errorf("event with %s attributes succeeded", name)
		}
	}

	return *incomplete.SpanID
}

func applicationEvents(
	t *testing.T,
	recording *termtrace.Recording,
	metadataOnly bool,
) []termtrace.EventProjection {
	t.Helper()

	page, err := recording.Events().Page(within(t, 30*time.Second), termtrace.EventPageOptions{
		Limit:        1000,
		MetadataOnly: metadataOnly,
		EventTypes:   []string{"application"},
	})
	fatal(t, err)

	return page.Events
}

func checkApplicationEvents(
	t *testing.T,
	client *termtrace.Client,
	recording *termtrace.Recording,
	incomplete string,
) {
	t.Helper()

	want := diagnosticsFixture(t).ApplicationAttributes
	started, ended, fixtureFound := false, false, false

	for _, projection := range applicationEvents(t, recording, false) {
		event, ok := projection.Event.(termtrace.EventApplication)
		if !ok {
			t.Fatalf("application page holds %T", projection.Event)
		}

		data := event.Data
		if strings.HasPrefix(data.Name, "unsafe") {
			t.Errorf("a rejected event was recorded: %+v", data)
		}

		if data.Name == "fixture" {
			fixtureFound = true

			attributes := map[string]any{}

			for key, raw := range data.Attributes {
				var value any
				fatal(t, termtrace.DecodeJSON(raw, &value))
				attributes[key] = value
			}

			if !reflect.DeepEqual(attributes, want) {
				t.Errorf("fixture attributes = %v, want %v", attributes, want)
			}
		}

		if data.SpanID != nil && *data.SpanID == incomplete {
			started = started || data.Kind == termtrace.ApplicationEventKindSpanStart
			ended = ended || data.Kind == termtrace.ApplicationEventKindSpanEnd
		}
	}

	if !fixtureFound || !started || ended {
		t.Errorf(
			"fixture event %v, incomplete span started %v, ended %v",
			fixtureFound,
			started,
			ended,
		)
	}

	for _, projection := range applicationEvents(t, recording, true) {
		if _, ok := projection.Metadata.(termtrace.EventMetadataApplication); !ok ||
			projection.Event != nil {
			t.Errorf("metadata-only projection = %+v", projection)
		}
	}

	ctx := within(t, 30*time.Second)

	emitter, err := client.Instrumentation(ctx, string(recording.Path()))
	if err == nil {
		_, err = emitter.Event(ctx, "after-completion", nil)
		_ = emitter.Close(ctx)
	}

	if err == nil {
		t.Error("an event after completion succeeded")
	}
}

// failingBinary serves the SDK through the real CLI and fails every other invocation, such as a
// restart's recorder launch.
func failingBinary(t *testing.T) string {
	t.Helper()

	path := filepath.Join(t.TempDir(), "termtrace")
	script := "#!/bin/sh\nif [ \"$1\" = sdk-server ]; then exec " + quote(binary(t)) +
		" \"$@\"; fi\nexit 1\n"
	fatal(t, os.WriteFile(path, []byte(script), 0o700))

	return path
}

func clientWithBinary(t *testing.T, path string) *termtrace.Client {
	t.Helper()

	client, err := termtrace.New(t.Context(), termtrace.Options{
		Binary:       path,
		EvidenceRoot: filepath.Join(t.TempDir(), "sessions"),
	})
	fatal(t, err)

	t.Cleanup(func() {
		if err := client.Close(); err != nil {
			t.Errorf("close client: %v", err)
		}
	})

	return client
}

func quote(text string) string {
	return "'" + strings.ReplaceAll(text, "'", `'\''`) + "'"
}

func semanticStates(page termtrace.TimelinePage) []termtrace.TimelineEntry {
	var states []termtrace.TimelineEntry

	for _, entry := range page.Entries {
		if _, ok := entry.Item.(termtrace.TimelineItemSemanticState); ok {
			states = append(states, entry)
		}
	}

	return states
}

func restartFacts(t *testing.T, err error) (*termtrace.ErrorData, termtrace.RestartFailureFacts) {
	t.Helper()

	failure := termtraceError(t, err)
	if failure.Facts == nil {
		t.Fatalf("restart failure has no facts: %v", err)
	}

	management, ok := failure.Facts.Management.(termtrace.ManagementFailureRegistryRestart)
	if !ok {
		t.Fatalf("management = %T, want registry_restart: %v", failure.Facts.Management, err)
	}

	return failure.Facts, management.Facts
}

func TestAnalysisSaveBundleAndCacheInvariance(t *testing.T) {
	want := diagnosticsFixture(t)
	session := completed(t)
	client, root, recording := session.client, session.root, session.recording
	ctx := within(t, 60*time.Second)
	analysis := recording.Analysis()
	evidence := recording.Evidence()

	attached, err := client.Attach(ctx, string(recording.Path()))
	fatal(t, err)

	before := topLevelHashes(t, string(recording.Path()))

	first, err := attached.Screen().Observe(ctx, termtrace.ObserveOptions{})
	fatal(t, err)

	observation := canonicalReplay(first)
	again, err := attached.Screen().Observe(ctx, termtrace.ObserveOptions{})
	fatal(t, err)

	if !sameJSON(t, canonicalReplay(again), observation) {
		t.Error("a repeated observation of a completed session changed")
	}

	page, err := analysis.Timeline(ctx, termtrace.TimelineOptions{Limit: 1000, Controls: true})
	fatal(t, err)

	states := semanticStates(page)
	if len(states) == 0 {
		t.Fatal("timeline has no semantic states")
	}

	last := states[len(states)-1]
	lastState, _ := last.Item.(termtrace.TimelineItemSemanticState)

	diff, err := analysis.Diff(
		ctx,
		termtrace.StatePointStateId{StateID: 0},
		termtrace.StatePointLatestPresented{},
	)
	fatal(t, err)

	if !diff.Changed {
		t.Error("the initial state equals the latest presented state")
	}

	explain, err := analysis.Explain(ctx, termtrace.ExplainOptions{})
	fatal(t, err)

	if explain.SourceEventCount == 0 || explain.SessionID != recording.ID() {
		t.Errorf("explain = %d events, session %s", explain.SourceEventCount, explain.SessionID)
	}

	diagnosis, err := analysis.Diagnose(ctx, termtrace.DiagnoseOptions{})
	fatal(t, err)

	if diagnosis.SessionID != recording.ID() || diagnosis.Findings == nil {
		t.Errorf("diagnose = session %s, findings %v", diagnosis.SessionID, diagnosis.Findings)
	}

	lastAction, err := analysis.LastAction(ctx, 0)
	fatal(t, err)

	if lastAction.Diagnostics.SessionID != recording.ID() {
		t.Errorf("last action diagnostics session = %s", lastAction.Diagnostics.SessionID)
	}

	markers, err := analysis.Markers(ctx, 0)
	fatal(t, err)

	if !slices.ContainsFunc(
		markers,
		func(marker termtrace.MarkerRecord) bool { return marker.Label == "cut" },
	) {
		t.Errorf("markers = %+v, want cut", markers)
	}

	summary, err := analysis.ReviewSummary(ctx)
	fatal(t, err)

	if summary.CanonicalSession != recording.Path() {
		t.Errorf("review summary session = %s", summary.CanonicalSession)
	}

	search, err := analysis.ReviewSearch(ctx, "READY")
	fatal(t, err)

	if len(search.Matches) == 0 {
		t.Error("review search found no READY")
	}

	export, err := analysis.ReviewExport(ctx, lastState.StateID, filepath.Join(root, "review"))
	fatal(t, err)

	if export.CanonicalSession != recording.Path() || export.StateID != lastState.StateID {
		t.Errorf("review export = %s state %d", export.CanonicalSession, export.StateID)
	}

	allFormats := filepath.Join(root, "all-formats")
	saved, err := evidence.Save(ctx, allFormats, want.SaveFormats, termtrace.SaveOptions{})
	fatal(t, err)

	if len(saved.Manifest.Files) != len(want.SaveFormats) ||
		len(saved.Manifest.SemanticImageResources) == 0 ||
		saved.Manifest.Sensitivity != "terminal_screen_may_contain_secrets" {
		t.Errorf("save manifest = %+v", saved.Manifest)
	}

	png := slices.IndexFunc(
		saved.Manifest.Files,
		func(file termtrace.SavedFile) bool { return file.Path == "screen.png" },
	)
	if png < 0 || saved.Manifest.Files[png].TruthLayer == nil ||
		*saved.Manifest.Files[png].TruthLayer != "semantic_rendering" {
		t.Errorf("screen.png manifest entry = %d in %+v", png, saved.Manifest.Files)
	}

	published := topLevelHashes(t, allFormats)

	_, err = evidence.Save(
		ctx,
		allFormats,
		[]termtrace.SaveFormat{termtrace.SaveFormatTxt},
		termtrace.SaveOptions{},
	)
	if err == nil {
		t.Error("save over a published directory succeeded")
	}

	if !maps.Equal(topLevelHashes(t, allFormats), published) {
		t.Error("a rejected save changed the published directory")
	}

	points := map[string]termtrace.StatePoint{
		"latest_presented": termtrace.StatePointLatestPresented{},
		"state_id":         termtrace.StatePointStateId{StateID: lastState.StateID},
		"state_hash":       termtrace.StatePointStateHash{StateHash: lastState.StateHash},
		"monotonic_ns":     termtrace.StatePointMonotonicNs{MonotonicNs: last.MonotonicNs},
		"marker":           termtrace.StatePointMarker{Label: "cut"},
	}
	if last.EventSeq != nil {
		points["event_seq"] = termtrace.StatePointEventSeq{EventSeq: *last.EventSeq}
	}

	for _, selector := range want.Selectors {
		point, ok := points[selector]
		if !ok {
			t.Errorf("no state point for selector %s", selector)
			continue
		}

		report, err := evidence.Save(
			ctx,
			filepath.Join(root, selector),
			[]termtrace.SaveFormat{termtrace.SaveFormatTxt},
			termtrace.SaveOptions{Point: point},
		)
		fatal(t, err)

		if !sameJSON(t, report.Manifest.StatePoint, point) {
			t.Errorf("%s: saved state point = %+v", selector, report.Manifest.StatePoint)
		}
	}

	cast, err := evidence.Asciicast(ctx, filepath.Join(root, "session.cast"))
	fatal(t, err)

	if info, err := os.Stat(string(cast)); err != nil || info.Size() == 0 {
		t.Errorf("asciicast %s: %v", cast, err)
	}

	other := launch(t, client, []string{"/bin/true"}, termtrace.LaunchOptions{
		Environment: &termtrace.EnvironmentSpec{Base: termtrace.EnvironmentBaseClear},
	})
	fatal(t, other.Stop(ctx))

	for _, profile := range want.Profiles {
		destination := filepath.Join(root, string(profile))

		report, err := evidence.Bundle(ctx, destination, profile)
		fatal(t, err)

		if report.Profile != profile ||
			report.IncludesCanonicalEvidence != (profile == termtrace.BundleProfileForensic) {
			t.Errorf("%s: bundle = %s, canonical %v", profile, report.Profile,
				report.IncludesCanonicalEvidence)
		}

		for _, member := range report.Members {
			if member.Sensitivity == "" {
				t.Errorf("%s: member %s has no sensitivity", profile, member.Path)
			}
		}

		pruned, err := analysis.CachePrune(ctx)
		fatal(t, err)

		if pruned.Remaining.Entries != 0 {
			t.Errorf("%s: cache entries after prune = %d", profile, pruned.Remaining.Entries)
		}

		current, err := attached.Screen().Observe(ctx, termtrace.ObserveOptions{})
		fatal(t, err)

		if !sameJSON(t, canonicalReplay(current), observation) {
			t.Errorf("%s: observation changed after cache prune", profile)
		}

		verified, err := evidence.VerifyBundle(ctx, destination, profile)
		fatal(t, err)

		if !sameJSON(t, verified, report) {
			t.Errorf("%s: verified bundle differs from the written one", profile)
		}

		if _, err := client.Evidence(string(other.Path())).
			VerifyBundle(ctx, destination, profile); err == nil {
			t.Errorf("%s: bundle verified against another session", profile)
		}

		wrong := termtrace.BundleProfileScreen
		if profile == termtrace.BundleProfileScreen {
			wrong = termtrace.BundleProfileDebug
		}

		if _, err := evidence.VerifyBundle(ctx, destination, wrong); err == nil {
			t.Errorf("%s: bundle verified as %s", profile, wrong)
		}

		if _, err := evidence.Bundle(ctx, destination, profile); err == nil {
			t.Errorf("%s: bundle over an existing directory succeeded", profile)
		}

		screen, err := os.OpenFile(
			filepath.Join(destination, "screen.txt"),
			os.O_APPEND|os.O_WRONLY,
			0,
		)
		fatal(t, err)
		_, err = screen.WriteString("TAMPER")
		fatal(t, errors.Join(err, screen.Close()))

		if _, err := evidence.VerifyBundle(ctx, destination, profile); err == nil {
			t.Errorf("%s: tampered bundle verified", profile)
		}
	}

	if _, err := evidence.Bundle(
		ctx,
		filepath.Join(root, "bad-profile"),
		termtrace.BundleProfile("forensik"),
	); err == nil {
		t.Error("bundle with an unknown profile succeeded")
	}

	if _, err := recording.Integrity().Verify(ctx, termtrace.VerifyOptions{
		Level: termtrace.VerificationLevel("signatur"),
	}); err == nil {
		t.Error("verify with an unknown level succeeded")
	}

	if _, err := analysis.Timeline(ctx, termtrace.TimelineOptions{
		Limit: 100,
		Range: &termtrace.SourceRange{StartMonotonicNs: 2, EndMonotonicNs: 1},
	}); err == nil {
		t.Error("timeline with an inverted range succeeded")
	}

	if after := topLevelHashes(t, string(recording.Path())); !maps.Equal(after, before) {
		t.Errorf("source files changed: %v -> %v", before, after)
	}

	checkApplicationEvents(t, client, recording, session.incomplete)
}

func TestSignatureOutcomesStayDistinct(t *testing.T) {
	keygen, err := exec.LookPath("ssh-keygen")
	if err != nil {
		t.Skip("ssh-keygen is not on PATH")
	}

	recording := completed(t).recording
	ctx := within(t, 60*time.Second)
	integrity := recording.Integrity()
	signature := termtrace.VerifyOptions{Level: termtrace.VerificationLevelSignature}

	content, err := integrity.Verify(ctx, termtrace.VerifyOptions{
		Level: termtrace.VerificationLevelContent,
	})
	fatal(t, err)

	if !content.Valid {
		t.Errorf("content verification = %+v", content.Findings)
	}

	unsigned, err := integrity.Verify(ctx, signature)
	fatal(t, err)

	if unsigned.Valid || unsigned.SignatureReceiptPresent {
		t.Errorf(
			"unsigned = valid %v, receipt %v",
			unsigned.Valid,
			unsigned.SignatureReceiptPresent,
		)
	}

	before := topLevelHashes(t, string(recording.Path()))
	keys := t.TempDir()
	key := filepath.Join(keys, "key")

	if output, err := exec.CommandContext(
		ctx, keygen, "-q", "-t", "ed25519", "-N", "", "-f", key,
	).CombinedOutput(); err != nil {
		t.Fatalf("ssh-keygen: %v: %s", err, output)
	}

	public, err := os.ReadFile(key + ".pub")
	fatal(t, err)

	policy := filepath.Join(keys, "allowed")
	wrong := filepath.Join(keys, "wrong")

	fatal(t, os.WriteFile(policy, append([]byte("tester "), public...), 0o600))
	fatal(t, os.WriteFile(wrong, append([]byte("someone-else "), public...), 0o600))

	receipt, err := integrity.Sign(ctx, key, "tester")
	fatal(t, err)

	untrusted, err := integrity.Verify(ctx, signature)
	fatal(t, err)

	if !untrusted.SignatureCryptographicValid || untrusted.SignatureSignerTrusted ||
		untrusted.Valid {
		t.Errorf("untrusted = %+v", untrusted)
	}

	trustedOptions := termtrace.VerifyOptions{
		Level:          termtrace.VerificationLevelSignature,
		AllowedSigners: policy,
	}

	trusted, err := integrity.Verify(ctx, trustedOptions)
	fatal(t, err)

	if !trusted.Valid || !trusted.SessionAuthenticityEstablished {
		t.Errorf("trusted = %+v", trusted)
	}

	rejected, err := integrity.Verify(ctx, termtrace.VerifyOptions{
		Level:          termtrace.VerificationLevelSignature,
		AllowedSigners: wrong,
	})
	fatal(t, err)

	if !rejected.SignatureCryptographicValid || rejected.SignatureSignerTrusted {
		t.Errorf("wrong signer = %+v", rejected)
	}

	original, err := os.ReadFile(string(receipt))
	fatal(t, err)
	fatal(t, os.WriteFile(string(receipt), []byte("{"), 0o600))

	if damaged, err := integrity.Verify(ctx, trustedOptions); err != nil || damaged.Valid {
		t.Errorf("damaged receipt = valid %v, %v", damaged.Valid, err)
	}

	// The receipt holds u128 timestamps, so edit only the base64 signature token as text.
	token := regexp.MustCompile(`("value"\s*:\s*")([A-Za-z0-9+/=]+)(")`)
	if !token.Match(original) {
		t.Fatalf("receipt has no signature value: %s", original)
	}

	flipped := token.ReplaceAllFunc(original, func(match []byte) []byte {
		parts := token.FindSubmatch(match)
		raw, err := base64.StdEncoding.DecodeString(string(parts[2]))
		fatal(t, err)

		raw[len(raw)/2] ^= 1

		return slices.Concat(parts[1], []byte(base64.StdEncoding.EncodeToString(raw)), parts[3])
	})
	fatal(t, os.WriteFile(string(receipt), flipped, 0o600))

	tampered, err := integrity.Verify(ctx, trustedOptions)
	fatal(t, err)

	if tampered.Valid || tampered.SessionAuthenticityEstablished {
		t.Errorf("tampered = %+v", tampered)
	}

	fatal(t, os.WriteFile(string(receipt), original, 0o600))

	if restored, err := integrity.Verify(ctx, trustedOptions); err != nil || !restored.Valid {
		t.Errorf("restored receipt = valid %v, %v", restored.Valid, err)
	}

	after := topLevelHashes(t, string(recording.Path()))
	for name, hash := range before {
		if after[name] != hash {
			t.Errorf("signing changed %s", name)
		}
	}
}

func TestRegistryExactU128CorruptionAndGcProtection(t *testing.T) {
	want := diagnosticsFixture(t)
	state := filepath.Join(t.TempDir(), "state")
	ctx := within(t, 60*time.Second)

	client, err := termtrace.New(t.Context(), termtrace.Options{
		Binary:       binary(t),
		EvidenceRoot: filepath.Join(state, "sessions"),
	})
	fatal(t, err)
	t.Cleanup(func() {
		if err := client.Close(); err != nil {
			t.Errorf("close client: %v", err)
		}
	})

	registry := client.Registry(state)

	absent, err := registry.Resolve(ctx, "absent")
	fatal(t, err)

	if absent != nil {
		t.Errorf("absent alias = %+v", absent)
	}

	fatal(t, os.MkdirAll(filepath.Join(state, "sessions"), 0o700))

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf LIVE; read line"},
		termtrace.LaunchOptions{},
	)
	path := string(recording.Path())
	_, err = recording.Output().WaitFor(ctx, []byte("LIVE"), 5*time.Second)
	fatal(t, err)
	_, err = registry.Register(ctx, "named", path)
	fatal(t, err)

	lifecycle, err := registry.Lifecycle(ctx, "named")
	fatal(t, err)

	if lifecycle.Lifecycle != termtrace.SessionPhaseLive {
		t.Errorf("lifecycle = %s", lifecycle.Lifecycle)
	}

	live, err := registry.Gc(
		ctx,
		removeEverything(true),
	)
	fatal(t, err)

	if live.RemovedSessions != 0 {
		t.Errorf("gc removed %d sessions while one is live", live.RemovedSessions)
	}

	if _, err := os.Stat(path); err != nil {
		t.Fatal(err)
	}

	fatal(t, recording.Stop(ctx))

	before := topLevelHashes(t, path)
	file := filepath.Join(state, "registry", "named.json")
	data, err := os.ReadFile(file)
	fatal(t, err)

	timestamp := regexp.MustCompile(`("registered_unix_ns"\s*:\s*)\d+`)
	if !timestamp.Match(data) {
		t.Fatalf("registry entry has no registered_unix_ns: %s", data)
	}

	maximum := timestamp.ReplaceAll(data, []byte("${1}"+want.U128Max))
	fatal(t, os.WriteFile(file, maximum, 0o600))

	resolved, err := registry.Resolve(ctx, "named")
	fatal(t, err)

	if resolved == nil || resolved.RegisteredUnixNs.String() != want.U128Max {
		t.Errorf("resolved = %+v", resolved)
	}

	listed, err := registry.List(ctx, true)
	fatal(t, err)

	if len(listed) != 1 || listed[0].RegisteredUnixNs.String() != want.U128Max {
		t.Errorf("listed = %+v", listed)
	}

	fatal(t, os.WriteFile(file, []byte("{"), 0o600))

	if _, err := registry.Resolve(ctx, "named"); err == nil {
		t.Error("a corrupt alias resolved")
	}

	blocked, err := registry.Gc(
		ctx,
		removeEverything(true),
	)
	fatal(t, err)

	if blocked.SkippedCorruptRegistryEntries != 1 || blocked.RemovedSessions != 0 {
		t.Errorf("gc with a corrupt entry = skipped %d, removed %d",
			blocked.SkippedCorruptRegistryEntries, blocked.RemovedSessions)
	}

	if _, err := os.Stat(path); err != nil {
		t.Fatal(err)
	}

	fatal(t, os.WriteFile(file, maximum, 0o600))

	plan, err := registry.Gc(ctx, removeEverything(false))
	fatal(t, err)

	if plan.ChangesApplied {
		t.Error("a planning gc applied changes")
	}

	_, err = registry.Remove(ctx, "named")
	fatal(t, err)

	if after := topLevelHashes(t, path); !maps.Equal(after, before) {
		t.Errorf("registry operations changed the session: %v -> %v", before, after)
	}

	candidates, err := registry.Gc(ctx, removeEverything(false))
	fatal(t, err)

	if len(candidates.Candidates) == 0 {
		t.Error("an unprotected completed session is not a gc candidate")
	}

	removed, err := registry.Gc(ctx, removeEverything(true))
	fatal(t, err)

	if removed.RemovedSessions != 1 {
		t.Errorf("gc removed %d sessions, want 1", removed.RemovedSessions)
	}

	if _, err := os.Stat(path); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("gc kept the session: %v", err)
	}
}

func removeEverything(apply bool) termtrace.GcOptions {
	return termtrace.GcOptions{
		OlderThanDays:     new(uint64(0)),
		KeepLatest:        new(uint64(0)),
		MaxCompletedBytes: new(uint64(0)),
		Apply:             apply,
	}
}

func TestRestartReportAndFailedReadinessKeepPredecessor(t *testing.T) {
	session := completed(t)
	client, root, recording := session.client, session.root, session.recording
	ctx := within(t, 60*time.Second)
	state := filepath.Join(root, "restart-state")
	registry := client.Registry(state)
	path := string(recording.Path())

	predecessor, err := registry.Register(ctx, "demo", path)
	fatal(t, err)

	before := topLevelHashes(t, path)
	environment := &termtrace.EnvironmentSpec{
		Base: termtrace.EnvironmentBaseClear,
		Set:  map[string]termtrace.Base64Bytes{"FIXTURE_NEW": []byte("yes")},
	}

	_, err = registry.Restart(ctx, "demo", termtrace.RestartOptions{
		Options:     termtrace.RestartConfiguration{Command: [][]byte{[]byte("/bin/true")}},
		Environment: environment,
		Timeout:     5 * time.Second,
	})
	if err == nil || !strings.Contains(err.Error(), "recoverable orphan") {
		t.Errorf("restart with an unpermitted environment change = %v", err)
	}

	resolved, err := registry.Resolve(ctx, "demo")
	fatal(t, err)

	if resolved == nil || resolved.SessionID != predecessor.SessionID {
		t.Errorf("alias after a failed restart = %+v", resolved)
	}

	successor, err := registry.Restart(ctx, "demo", termtrace.RestartOptions{
		Options: termtrace.RestartConfiguration{
			Command:                [][]byte{[]byte("/bin/true")},
			Cols:                   new(66),
			AllowEnvironmentChange: true,
		},
		Environment: environment,
		Timeout:     5 * time.Second,
	})
	fatal(t, err)

	if !successor.Ready || successor.Predecessor.SessionID != predecessor.SessionID ||
		successor.Successor.RegisteredUnixNs.Big().Sign() == 0 {
		t.Errorf("restart = %+v", successor)
	}

	resolved, err = registry.Resolve(ctx, "demo")
	fatal(t, err)

	if resolved == nil || resolved.SessionID != successor.Successor.SessionID {
		t.Errorf("alias after restart = %+v", resolved)
	}

	_, err = registry.Register(ctx, "failure", path)
	fatal(t, err)

	failing := clientWithBinary(t, failingBinary(t)).Registry(state)

	_, err = failing.Restart(
		ctx,
		"failure",
		termtrace.RestartOptions{Timeout: 500 * time.Millisecond},
	)
	if err == nil || !strings.Contains(err.Error(), "recoverable orphan") {
		t.Errorf("restart with a failing recorder = %v", err)
	}

	resolved, err = registry.Resolve(ctx, "failure")
	fatal(t, err)

	if resolved == nil || resolved.SessionID != predecessor.SessionID {
		t.Errorf("alias after failed readiness = %+v", resolved)
	}

	if after := topLevelHashes(t, path); !maps.Equal(after, before) {
		t.Errorf("restart changed the predecessor: %v -> %v", before, after)
	}
}

func TestHiddenStateSelectionAndSystemSurfaces(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 60*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", `printf '\033[?2026hHIDDEN\033[?2026lVISIBLE'`},
		termtrace.LaunchOptions{},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("VISIBLE"), 5*time.Second)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	page, err := recording.Analysis().Timeline(
		ctx,
		termtrace.TimelineOptions{Limit: 1000, IncludeHidden: true},
	)
	fatal(t, err)

	var hidden *termtrace.TimelineItemSemanticState

	for _, entry := range semanticStates(page) {
		if state, _ := entry.Item.(termtrace.TimelineItemSemanticState); !state.Presented {
			hidden = &state
			break
		}
	}

	if hidden == nil {
		t.Fatal("timeline has no hidden state")
	}

	point := termtrace.StatePointStateId{StateID: hidden.StateID}
	txt := []termtrace.SaveFormat{termtrace.SaveFormatTxt}
	notPublished := filepath.Join(root, "not-published")

	if _, err := recording.Evidence().
		Save(ctx, notPublished, txt, termtrace.SaveOptions{Point: point}); err == nil {
		t.Error("saving a hidden state without opting in succeeded")
	}

	if _, err := os.Stat(notPublished); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("rejected save left %s: %v", notPublished, err)
	}

	saved, err := recording.Evidence().Save(
		ctx,
		filepath.Join(root, "hidden"),
		txt,
		termtrace.SaveOptions{Point: point, IncludeHidden: true},
	)
	fatal(t, err)

	if !sameJSON(t, saved.Manifest.StatePoint, point) {
		t.Errorf("hidden state point = %+v", saved.Manifest.StatePoint)
	}

	versions, err := client.Versions(ctx)
	fatal(t, err)

	if versions.TargetOs != "linux" {
		t.Errorf("target os = %s", versions.TargetOs)
	}

	schemas, err := client.Schemas(ctx)
	fatal(t, err)

	if !slices.Contains(schemas, "recovery-report.schema.json") {
		t.Errorf("schemas = %v", schemas)
	}

	for _, name := range schemas {
		schema, err := client.Schema(ctx, name)
		fatal(t, err)

		if schema.Kind() != '{' {
			t.Errorf("schema %s is a %v, want an object", name, schema.Kind())
		}
	}

	if _, err := client.Schema(ctx, "no-such-schema"); err == nil {
		t.Error("an unknown schema resolved")
	}

	report, err := client.Capabilities(ctx)
	fatal(t, err)

	signing := slices.IndexFunc(
		report.Capabilities,
		func(capability termtrace.Capability) bool { return capability.ID == "receipt-signing" },
	)
	if signing < 0 || report.Capabilities[signing].Status != termtrace.CapabilityStatusReady {
		t.Errorf("receipt-signing capability = %d in %+v", signing, report.Capabilities)
	}
}

func TestLiveRestartFailureKeepsPredecessorFacts(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 60*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf LIVE; read line"},
		termtrace.LaunchOptions{},
	)
	path := string(recording.Path())
	registry := clientWithBinary(t, failingBinary(t)).Registry(filepath.Join(root, "registry"))

	_, err := recording.Output().WaitFor(ctx, []byte("LIVE"), 5*time.Second)
	fatal(t, err)

	old, err := registry.Register(ctx, "live", path)
	fatal(t, err)

	_, err = registry.Restart(ctx, "live", termtrace.RestartOptions{Timeout: 5 * time.Second})
	data, facts := restartFacts(t, err)

	if data.Scope != termtrace.FailureScopeManagement || data.Effect != nil ||
		data.Retry != nil || data.RequestID != nil ||
		data.Guidance == nil || *data.Guidance != termtrace.ManagementGuidanceInspectNoAutomaticRetry ||
		data.Kind != "recorder_startup" || data.Code != "recorder_startup" {
		t.Errorf("restart failure = %+v", data)
	}

	if facts.Phase != termtrace.RestartFailurePhaseSuccessorLaunched ||
		string(facts.PredecessorPath) != path {
		t.Errorf("phase %s, predecessor %q", facts.Phase, facts.PredecessorPath)
	}

	if facts.PredecessorCompletion == nil ||
		facts.PredecessorCompletion.SessionID != old.SessionID ||
		facts.PredecessorCompletion.EventCount == 0 {
		t.Errorf("predecessor completion = %+v", facts.PredecessorCompletion)
	}

	successor := facts.Successor
	if successor == nil || !successor.Launched || successor.Readiness == nil ||
		*successor.Readiness != termtrace.SuccessorReadinessExited ||
		len(successor.Session) == 0 || len(successor.StdoutLog) == 0 ||
		len(successor.StderrLog) == 0 {
		t.Errorf("successor = %+v", successor)
	}

	resolved, err := registry.Resolve(ctx, "live")
	fatal(t, err)

	if resolved == nil || resolved.SessionID != old.SessionID {
		t.Errorf("alias after failed restart = %+v", resolved)
	}

	done, err := client.Registry(filepath.Join(root, "completed")).Register(ctx, "done", path)
	fatal(t, err)

	if done.RecorderProcessStartTicks != nil {
		t.Errorf("completed registration has recorder start ticks %d",
			*done.RecorderProcessStartTicks)
	}
}

type restartOutcome struct {
	report termtrace.RestartReport
	err    error
}

func TestRestartAliasRaceKeepsOtherOwner(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 60*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf LIVE; read line"},
		termtrace.LaunchOptions{},
	)
	other := launch(t, client, []string{"/bin/true"}, termtrace.LaunchOptions{})
	fatal(t, other.Stop(ctx))

	state := filepath.Join(root, "registry")
	plain := client.Registry(state)
	entered := filepath.Join(root, "entered")
	release := filepath.Join(root, "release")
	helper := filepath.Join(root, "helper")
	script := "#!/bin/sh\n: > " + quote(entered) + "\nwhile [ ! -e " + quote(release) +
		" ]; do sleep 0.01; done\nexec " + quote(binary(t)) + " \"$@\"\n"
	fatal(t, os.WriteFile(helper, []byte(script), 0o700))

	fatal(t, os.WriteFile(release, nil, 0o600))
	held := clientWithBinary(t, helper).Registry(state)
	fatal(t, os.Remove(release))
	fatal(t, os.Remove(entered))

	_, err := recording.Output().WaitFor(ctx, []byte("LIVE"), 5*time.Second)
	fatal(t, err)

	old, err := plain.Register(ctx, "race", string(recording.Path()))
	fatal(t, err)

	outcome := make(chan restartOutcome, 1)

	go func() {
		report, err := held.Restart(ctx, "race", termtrace.RestartOptions{
			Options: termtrace.RestartConfiguration{
				Command:                [][]byte{[]byte("/bin/true")},
				AllowEnvironmentChange: true,
			},
			Environment: &termtrace.EnvironmentSpec{Base: termtrace.EnvironmentBaseClear},
			Timeout:     5 * time.Second,
		})
		outcome <- restartOutcome{report: report, err: err}
	}()

	t.Cleanup(func() {
		_ = os.WriteFile(release, nil, 0o600)
	})

	for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); {
		if _, err := os.Stat(entered); err == nil {
			break
		}

		time.Sleep(10 * time.Millisecond)
	}

	if _, err := os.Stat(entered); err != nil {
		fatal(t, errors.Join(os.WriteFile(release, nil, 0o600), err))
	}

	_, err = plain.Remove(ctx, "race")
	fatal(t, err)

	winner, err := plain.Register(ctx, "race", string(other.Path()))
	fatal(t, err)
	fatal(t, os.WriteFile(release, nil, 0o600))

	result := <-outcome
	if result.err == nil {
		t.Fatalf("restart retargeted a taken alias: %+v", result.report)
	}

	data, facts := restartFacts(t, result.err)
	if data.Scope != termtrace.FailureScopeManagement || data.Effect != nil ||
		data.Retry != nil || data.Kind != "engine" {
		t.Errorf("restart failure = %+v", data)
	}

	if facts.Phase != termtrace.RestartFailurePhaseAliasRetarget ||
		facts.PredecessorCompletion == nil ||
		facts.PredecessorCompletion.SessionID != old.SessionID {
		t.Errorf("phase %s, predecessor %+v", facts.Phase, facts.PredecessorCompletion)
	}

	successor := facts.Successor
	if successor == nil || successor.Readiness == nil ||
		(*successor.Readiness != termtrace.SuccessorReadinessLive &&
			*successor.Readiness != termtrace.SuccessorReadinessCompleted) {
		t.Fatalf("successor = %+v", successor)
	}

	if _, err := os.Stat(filepath.Join(string(successor.Session), "session.json")); err != nil {
		t.Errorf("orphan successor: %v", err)
	}

	resolved, err := plain.Resolve(ctx, "race")
	fatal(t, err)

	if resolved == nil || resolved.SessionID != winner.SessionID {
		t.Errorf("alias owner = %+v, want %s", resolved, winner.SessionID)
	}
}

type bundleVerification struct {
	Session struct {
		Base64 []byte `json:"base64"`
	} `json:"session"`
	Valid     bool   `json:"valid"`
	SessionID string `json:"session_id"`
}

func TestNonUTF8SourceBundlesKeepExactPaths(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 60*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf BYTEBOUND"},
		termtrace.LaunchOptions{},
	)
	fatal(t, recording.Stop(ctx))

	other := launch(t, client, []string{"/bin/true"}, termtrace.LaunchOptions{})
	fatal(t, other.Stop(ctx))

	before := topLevelHashes(t, string(recording.Path()))
	moved := root + "/source-\xff"
	fatal(t, os.Rename(string(recording.Path()), moved))

	source, err := filepath.EvalSymlinks(moved)
	fatal(t, err)

	native, err := client.Integrity(source).Verify(ctx, termtrace.VerifyOptions{
		Level: termtrace.VerificationLevelContent,
	})
	fatal(t, err)

	if !native.Valid || string(native.Session) != source || native.SessionID == nil {
		t.Fatalf("native verification = %+v", native)
	}

	evidence := client.Evidence(source)

	for _, profile := range []termtrace.BundleProfile{
		termtrace.BundleProfileDebug,
		termtrace.BundleProfileForensic,
	} {
		destination := root + "/bundle-\xfe-" + string(profile)

		report, err := evidence.Bundle(ctx, destination, profile)
		fatal(t, err)

		verified, err := evidence.VerifyBundle(ctx, destination, profile)
		fatal(t, err)

		if !sameJSON(t, verified, report) {
			t.Errorf("%s: verified bundle differs from the written one", profile)
		}

		if _, err := client.Evidence(string(other.Path())).
			VerifyBundle(ctx, destination, profile); err == nil {
			t.Errorf("%s: bundle verified against another session", profile)
		}

		file := filepath.Join(destination, "verification.json")
		encoded, err := os.ReadFile(file)
		fatal(t, err)

		var value bundleVerification
		fatal(t, json.Unmarshal(encoded, &value))

		if !bytes.Equal(value.Session.Base64, []byte(source)) || !value.Valid ||
			value.SessionID != *native.SessionID {
			t.Errorf("%s: verification.json = %+v", profile, value)
		}

		fatal(t, os.WriteFile(file, append(slices.Clone(encoded), "TAMPER"...), 0o600))

		if _, err := evidence.VerifyBundle(ctx, destination, profile); err == nil {
			t.Errorf("%s: tampered verification.json verified", profile)
		}

		fatal(t, os.WriteFile(file, encoded, 0o600))

		restored, err := evidence.VerifyBundle(ctx, destination, profile)
		fatal(t, err)

		if !sameJSON(t, restored, report) {
			t.Errorf("%s: restored bundle differs from the written one", profile)
		}

		if after := topLevelHashes(t, source); !maps.Equal(after, before) {
			t.Errorf("%s: source changed: %v -> %v", profile, before, after)
		}
	}
}

func TestSystemCapabilityProbe(t *testing.T) {
	client, _ := newClient(t)

	report, err := client.Capabilities(within(t, 60*time.Second))
	fatal(t, err)

	if len(report.Capabilities) == 0 || report.TermtraceVersion != termtrace.Version {
		t.Errorf("capabilities = %+v", report)
	}

	for _, capability := range report.Capabilities {
		if capability.ID == "" || capability.Status == "" {
			t.Errorf("capability without id or status: %+v", capability)
		}
	}
}

func TestRegistryOperations(t *testing.T) {
	client, root := newClient(t)
	ctx := within(t, 30*time.Second)
	state := filepath.Join(root, "registry")
	registry := client.Registry(state)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo REGISTRY_GO; sleep 30"},
		termtrace.LaunchOptions{Rows: new(24), Cols: new(80)},
	)
	path := recording.Path()
	alias := "go-alias"

	entry, err := registry.Register(ctx, alias, string(path))
	fatal(t, err)

	if entry.Alias != alias || entry.CanonicalSession != path || entry.SessionID != recording.ID() {
		t.Errorf("registered = %+v", entry)
	}

	resolved, err := registry.Resolve(ctx, alias)
	fatal(t, err)

	if resolved == nil || resolved.CanonicalSession != path {
		t.Errorf("resolved = %+v", resolved)
	}

	listed, err := registry.List(ctx, true)
	fatal(t, err)

	if !slices.ContainsFunc(
		listed,
		func(item termtrace.RegistryListEntry) bool { return item.Alias == alias },
	) {
		t.Errorf("listed = %+v", listed)
	}

	removed, err := registry.Remove(ctx, alias)
	fatal(t, err)

	if removed.Alias != alias {
		t.Errorf("removed = %+v", removed)
	}

	if gone, err := registry.Resolve(ctx, alias); err != nil || gone != nil {
		t.Errorf("resolve after remove = %+v, %v", gone, err)
	}

	report, err := registry.Gc(
		ctx,
		termtrace.GcOptions{OlderThanDays: new(uint64(1)), KeepLatest: new(uint64(10))},
	)
	fatal(t, err)

	if report.StateRoot != termtrace.Path(state) || report.ChangesApplied {
		t.Errorf("gc = %+v", report)
	}

	fatal(t, recording.Stop(ctx))
}
