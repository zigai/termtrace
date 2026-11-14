package consumer

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"io/fs"
	"maps"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

func cursorText(t *testing.T) string {
	t.Helper()

	data, err := os.ReadFile(
		filepath.Join(repo(t), "fixtures", "conformance", "sdk", "evidence-pages.json"),
	)
	fatal(t, err)

	var document struct {
		EventCursor jsontext.Value `json:"event_cursor"`
	}

	fatal(t, json.Unmarshal(data, &document))
	fatal(t, document.EventCursor.Compact())

	return regexp.MustCompile(`"(\d+)"`).ReplaceAllString(string(document.EventCursor), "$1")
}

func roundTrip[T any](t *testing.T, value T) T {
	t.Helper()

	data, err := json.Marshal(value)
	fatal(t, err)

	var decoded T
	fatal(t, termtrace.DecodeJSON(data, &decoded))

	return decoded
}

func TestPageCursorsKeepExactNumbersAndPathBytes(t *testing.T) {
	text := cursorText(t)

	var cursor termtrace.EventPageCursor
	fatal(t, termtrace.DecodeJSON([]byte(text), &cursor))

	if cursor.NextEventSeq != math.MaxUint64 || cursor.Source.Device != math.MaxUint64 {
		t.Errorf("next seq %d, device %d", cursor.NextEventSeq, cursor.Source.Device)
	}

	if cursor.Source.Inode != 9007199254740993 {
		t.Errorf("inode %d", cursor.Source.Inode)
	}

	if !bytes.Equal(cursor.Source.SessionPath, []byte("/tmp/nonutf8-\xff")) {
		t.Errorf("session path %q", cursor.Source.SessionPath)
	}

	if again := roundTrip(t, cursor); !reflect.DeepEqual(again, cursor) {
		t.Errorf("round trip %+v, want %+v", again, cursor)
	}

	for _, token := range []string{"-1", "1.5", "18446744073709551616", `"1"`} {
		var invalid termtrace.EventPageCursor

		changed := strings.ReplaceAll(text, "18446744073709551615", token)
		if err := termtrace.DecodeJSON([]byte(changed), &invalid); err == nil {
			t.Errorf("token %s decoded to %+v", token, invalid)
		}
	}

	var output termtrace.OutputPageCursor
	fatal(t, termtrace.DecodeJSON(
		[]byte(`{"events":`+text+`,"byte_offset":18446744073709551615}`),
		&output,
	))

	if output.ByteOffset != math.MaxUint64 || !reflect.DeepEqual(output.Events, cursor) {
		t.Errorf("output cursor %+v", output)
	}

	if again := roundTrip(t, output); !reflect.DeepEqual(again, output) {
		t.Errorf("round trip %+v, want %+v", again, output)
	}
}

func TestPagesKeepExactBytesFollowPositionsAndResumeAfterCompletion(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 60*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", `printf 'A\000\377BC'; read line`},
		termtrace.LaunchOptions{},
	)
	output := recording.Output()
	events := recording.Events()

	matched, err := output.WaitFor(ctx, []byte{'A', 0, 0xff, 'B', 'C'}, 5*time.Second)
	fatal(t, err)

	if matched.OutputEndOffset != 5 {
		t.Errorf("output end offset %d", matched.OutputEndOffset)
	}

	first, err := output.Page(ctx, termtrace.OutputPageOptions{Limit: 3})
	fatal(t, err)

	if !bytes.Equal(first.Bytes, []byte{'A', 0, 0xff}) || !first.HasMore {
		t.Errorf("first page %q, has more %v", first.Bytes, first.HasMore)
	}

	position, err := events.Follow(ctx, 0)
	fatal(t, err)

	live, ok := position.(termtrace.EventFollowPositionLive)
	if !ok {
		t.Fatalf("follow while running = %T", position)
	}

	if live.Device != first.Cursor.Events.Source.Device {
		t.Errorf("live device %d, page device %d", live.Device, first.Cursor.Events.Source.Device)
	}

	_, err = recording.Mark(ctx, "evidence-page", termtrace.MutationOptions{})
	fatal(t, err)

	metadata, err := events.Page(ctx, termtrace.EventPageOptions{
		Limit:        1,
		MetadataOnly: true,
		EventTypes:   []string{"output"},
	})
	fatal(t, err)

	if len(metadata.Events) != 1 {
		t.Fatalf("metadata page has %d events", len(metadata.Events))
	}

	if metadata.Events[0].Event != nil || len(metadata.Events[0].BytePayloads) == 0 ||
		metadata.Events[0].BytePayloads[0].ByteCount != 5 {
		t.Errorf("metadata event %+v", metadata.Events[0])
	}

	fatal(t, recording.Stop(ctx))

	second, err := output.Page(ctx, termtrace.OutputPageOptions{Cursor: &first.Cursor, Limit: 2})
	fatal(t, err)

	if string(second.Bytes) != "BC" || second.HasMore {
		t.Errorf("second page %q, has more %v", second.Bytes, second.HasMore)
	}

	eof, err := output.Page(ctx, termtrace.OutputPageOptions{Cursor: &second.Cursor, Limit: 2})
	fatal(t, err)

	if len(eof.Bytes) != 0 || !reflect.DeepEqual(eof.Cursor, second.Cursor) {
		t.Errorf(
			"page at the end %q with cursor %+v, want cursor %+v",
			eof.Bytes,
			eof.Cursor,
			second.Cursor,
		)
	}

	position, err = events.Follow(ctx, 0)
	fatal(t, err)

	if _, ok := position.(termtrace.EventFollowPositionCompleted); !ok {
		t.Errorf("follow after stop = %T", position)
	}

	var (
		all    []termtrace.EventProjection
		cursor *termtrace.EventPageCursor
	)

	for {
		page, err := events.Page(ctx, termtrace.EventPageOptions{Cursor: cursor, Limit: 1})
		fatal(t, err)

		all = append(all, page.Events...)
		cursor = &page.Cursor

		if !page.HasMore {
			break
		}
	}

	marked := false

	for index, event := range all {
		if event.Seq != uint64(index) {
			t.Fatalf("event %d has seq %d", index, event.Seq)
		}

		if marker, ok := event.Event.(termtrace.EventMarker); ok &&
			marker.Label == "evidence-page" {
			marked = true
		}
	}

	if !marked {
		t.Error("the paged events have no evidence-page marker")
	}

	_, err = output.Page(ctx, termtrace.OutputPageOptions{Limit: 4194305})
	refused(t, err)

	_, err = events.Page(
		ctx,
		termtrace.EventPageOptions{Limit: 1, EventTypes: []string{"invented"}},
	)
	refused(t, err)

	_, err = recording.Integrity().Verify(ctx, termtrace.VerifyOptions{Level: "signatur"})
	refused(t, err)
}

func refused(t *testing.T, err error) {
	t.Helper()

	if failure := termtraceError(t, err); failure.Error() == "" {
		t.Error("the refusal has no message")
	}
}

func copyTree(t *testing.T, source string, destination string) {
	t.Helper()

	fatal(t, filepath.WalkDir(source, func(path string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}

		relative, err := filepath.Rel(source, path)
		if err != nil {
			return fmt.Errorf("relative path: %w", err)
		}

		target := filepath.Join(destination, relative)
		if entry.IsDir() {
			return os.Mkdir(target, 0o700)
		}

		data, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("read %q: %w", path, err)
		}

		info, err := entry.Info()
		if err != nil {
			return fmt.Errorf("stat %q: %w", path, err)
		}

		return os.WriteFile(target, data, info.Mode().Perm())
	}))
}

func hashes(t *testing.T, root string) map[string]string {
	t.Helper()

	result := map[string]string{}

	fatal(t, filepath.WalkDir(root, func(path string, entry fs.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return err
		}

		data, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("read %q: %w", path, err)
		}

		relative, err := filepath.Rel(root, path)
		if err != nil {
			return fmt.Errorf("relative path: %w", err)
		}

		digest := sha256.Sum256(data)
		result[relative] = hex.EncodeToString(digest[:])

		return nil
	}))

	return result
}

func exists(t *testing.T, path string) bool {
	t.Helper()

	_, err := os.Lstat(path)
	if err == nil {
		return true
	}

	if !os.IsNotExist(err) {
		t.Fatal(err)
	}

	return false
}

type recoveryDiagnostic struct {
	CompleteEvidence bool           `json:"complete_evidence"`
	SourceSession    termtrace.Path `json:"source_session"`
	Recovery         struct {
		Source    termtrace.Path `json:"source"`
		Recovered termtrace.Path `json:"recovered"`
	} `json:"recovery"`
}

func TestRecoveryBindsExactPathsAndPublishesAtomically(t *testing.T) {
	for _, names := range [][2]string{
		{"source-é", "dest-λ"},
		{"s\xff", "dest"},
		{"source", "d\xfe"},
		{"s\xff", "d\xfe"},
	} {
		t.Run(
			hex.EncodeToString([]byte(names[0]))+"-"+hex.EncodeToString([]byte(names[1])),
			func(t *testing.T) {
				checkRecovery(t, names[0], names[1])
			},
		)
	}
}

func checkRecovery(t *testing.T, sourceName string, destinationName string) {
	t.Helper()

	client, dir := newClient(t)
	ctx := within(t, 60*time.Second)

	root := filepath.Join(dir, "recovery")
	fatal(t, os.Mkdir(root, 0o700))

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf RECOVERY; read line"},
		termtrace.LaunchOptions{},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("RECOVERY"), 5*time.Second)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	source := root + "/" + sourceName
	destination := root + "/" + destinationName
	log := source + "/events.jsonl"

	copyTree(t, string(recording.Path()), source)

	file, err := os.OpenFile(log, os.O_APPEND|os.O_WRONLY, 0)
	fatal(t, err)
	_, err = file.WriteString(`{"torn":`)
	fatal(t, err)
	fatal(t, file.Close())

	before := hashes(t, source)

	report, err := client.Integrity(source).Recover(ctx, destination)
	fatal(t, err)

	if string(report.Source) != log || string(report.Recovered) != destination+"/events.jsonl" {
		t.Errorf("report paths %q -> %q", report.Source, report.Recovered)
	}

	if report.ValidEvents == 0 || report.DiscardedTail == nil {
		t.Errorf("valid events %d, discarded tail %v", report.ValidEvents, report.DiscardedTail)
	}

	if after := hashes(t, source); !reflect.DeepEqual(before, after) {
		t.Errorf("recovery changed the source: %v -> %v", before, after)
	}

	data, err := os.ReadFile(destination + "/recovery.json")
	fatal(t, err)

	var diagnostic recoveryDiagnostic
	fatal(t, termtrace.DecodeJSON(data, &diagnostic))

	if diagnostic.CompleteEvidence || string(diagnostic.SourceSession) != source ||
		string(diagnostic.Recovery.Source) != log ||
		string(diagnostic.Recovery.Recovered) != destination+"/events.jsonl" {
		t.Errorf("recovery.json %+v", diagnostic)
	}

	if exists(t, destination+"/completion.json") {
		t.Error("the recovered session claims completion")
	}

	page, err := client.Events(destination).Page(ctx, termtrace.EventPageOptions{Limit: 1000})
	fatal(t, err)

	if uint64(len(page.Events)) != report.ValidEvents || page.HasMore || page.IncompleteTail {
		t.Errorf(
			"recovered log has %d events, has more %v, incomplete tail %v",
			len(page.Events), page.HasMore, page.IncompleteTail,
		)
	}

	published := hashes(t, destination)

	_, err = client.Integrity(source).Recover(ctx, destination)
	refused(t, err)

	if !reflect.DeepEqual(published, hashes(t, destination)) {
		t.Error("a second recovery changed the published destination")
	}

	_, err = client.Integrity(source).Recover(ctx, source)
	refused(t, err)

	_, err = client.Integrity(source).Recover(ctx, source+"/nested-recovery")
	refused(t, err)

	alias := root + "/source-link"
	fatal(t, os.Symlink(source, alias))

	_, err = client.Integrity(source).Recover(ctx, alias+"/nested-recovery")
	refused(t, err)

	if exists(t, source+"/nested-recovery") {
		t.Error("recovery wrote inside the source")
	}

	if !reflect.DeepEqual(before, hashes(t, source)) {
		t.Error("a refused recovery changed the source")
	}

	logReport, err := client.Integrity(log).RecoverLog(ctx, root+"/p\xfd")
	fatal(t, err)

	if string(logReport.Source) != log || logReport.ValidEvents != report.ValidEvents {
		t.Errorf("log recovery %q with %d events", logReport.Source, logReport.ValidEvents)
	}

	fatal(t, os.WriteFile(log, []byte("invalid header\n"), 0o600))

	broken := hashes(t, source)
	absent := root + "/a\xfc"

	_, err = client.Integrity(source).Recover(ctx, absent)
	refused(t, err)

	if exists(t, absent) {
		t.Error("a failed recovery left its destination")
	}

	if !reflect.DeepEqual(broken, hashes(t, source)) {
		t.Error("a failed recovery changed the source")
	}

	entries, err := os.ReadDir(root)
	fatal(t, err)

	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".termtrace-recovery-") {
			t.Errorf("recovery left %q", entry.Name())
		}
	}
}

func TestSaveWritesEveryRequestedRepresentation(t *testing.T) {
	client, dir := newClient(t)
	ctx := within(t, 60*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo VISUAL_SAVE_GO; sleep 30"},
		termtrace.LaunchOptions{Rows: new(24), Cols: new(80)},
	)
	state, err := recording.Screen().WaitForText(ctx, "VISUAL_SAVE_GO", 5*time.Second)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	destination := filepath.Join(dir, "snapshot_out")
	formats := []termtrace.SaveFormat{
		termtrace.SaveFormatPng,
		termtrace.SaveFormatSvg,
		termtrace.SaveFormatHTML,
		termtrace.SaveFormatTxt,
		termtrace.SaveFormatJSON,
	}

	saved, err := recording.Evidence().Save(ctx, destination, formats, termtrace.SaveOptions{
		Point: termtrace.StatePointStateHash{StateHash: state.Hash},
	})
	fatal(t, err)

	if string(saved.Directory) != destination || saved.Manifest.StateHash != state.Hash ||
		saved.Manifest.SessionID != recording.ID() {
		t.Errorf(
			"saved %q for state %s of %s",
			saved.Directory,
			saved.Manifest.StateHash,
			saved.Manifest.SessionID,
		)
	}

	extensions := map[string][]byte{}

	for _, file := range saved.Manifest.Files {
		data, err := os.ReadFile(filepath.Join(destination, file.Path))
		fatal(t, err)

		if uint64(len(data)) != file.ByteCount {
			t.Errorf("%s has %d bytes, manifest says %d", file.Path, len(data), file.ByteCount)
		}

		extensions[filepath.Ext(file.Path)] = data
	}

	for _, extension := range []string{".png", ".svg", ".html", ".txt", ".json"} {
		if _, ok := extensions[extension]; !ok {
			t.Errorf("no %s file in %v", extension, slices.Sorted(maps.Keys(extensions)))
		}
	}

	if !bytes.HasPrefix(extensions[".png"], []byte("\x89PNG\r\n\x1a\n")) {
		t.Error("the png file is not a PNG")
	}

	if !bytes.Contains(extensions[".txt"], []byte("VISUAL_SAVE_GO")) {
		t.Errorf("text file %q", extensions[".txt"])
	}
}

func TestRawOutputPageAndWait(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo RAW_OUTPUT_GO; sleep 30"},
		termtrace.LaunchOptions{},
	)
	matched, err := recording.Output().WaitFor(ctx, []byte("RAW_OUTPUT_GO"), 5*time.Second)
	fatal(t, err)

	page, err := recording.Output().Page(ctx, termtrace.OutputPageOptions{Limit: 4096})
	fatal(t, err)

	if matched.OutputEndOffset > uint64(len(page.Bytes)) ||
		!bytes.HasSuffix(page.Bytes[:matched.OutputEndOffset], []byte("RAW_OUTPUT_GO")) {
		t.Errorf("match ends at %d in %q", matched.OutputEndOffset, page.Bytes)
	}
}

func TestEventLogPageStartsAtTheHeader(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo EVENT_GO_TEST; sleep 30"},
		termtrace.LaunchOptions{},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("EVENT_GO_TEST"), 5*time.Second)
	fatal(t, err)

	page, err := recording.Events().Page(ctx, termtrace.EventPageOptions{Limit: 50})
	fatal(t, err)

	if len(page.Events) == 0 || len(page.Events) > 50 {
		t.Fatalf("page has %d events", len(page.Events))
	}

	output := false

	for index, event := range page.Events {
		if event.Seq != uint64(index) {
			t.Fatalf("event %d has seq %d", index, event.Seq)
		}

		if event.EventType == "output" {
			output = true
		}
	}

	if !output {
		t.Error("the page has no output event")
	}
}

func TestReplayAnalysisOfACompletedSession(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 60*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh"},
		termtrace.LaunchOptions{Rows: new(24), Cols: new(80)},
	)
	state, err := recording.Screen().WaitForText(ctx, "$", 5*time.Second)
	fatal(t, err)

	action := recording.Actions().
		SendText("echo ACTION_FOR_ANALYSIS").
		SendKey("Enter").
		UntilScreen("ACTION_FOR_ANALYSIS", 5*time.Second)
	_, err = action.Run(ctx)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	analysis := recording.Analysis()

	explanation, err := analysis.Explain(ctx, termtrace.ExplainOptions{
		Target:  termtrace.StatePointStateHash{StateHash: state.Hash},
		Context: 1,
	})
	fatal(t, err)

	if explanation.TargetState.StateHash != state.Hash || explanation.ContextEvents != 1 ||
		explanation.SessionID != recording.ID() {
		t.Errorf(
			"explained state %s with context %d for %s",
			explanation.TargetState.StateHash, explanation.ContextEvents, explanation.SessionID,
		)
	}

	diagnosis, err := analysis.Diagnose(ctx, termtrace.DiagnoseOptions{BroadChangePercent: 20})
	fatal(t, err)

	if diagnosis.SessionID != recording.ID() || diagnosis.BroadChangeFraction != "20/100" ||
		diagnosis.SourceEventCount == 0 {
		t.Errorf("diagnosis %+v", diagnosis)
	}

	last, err := analysis.LastAction(ctx, 0)
	fatal(t, err)

	boundary := last.Inspection.Action
	if boundary.AtomCount != 2 || len(boundary.OperationIds) != 2 ||
		boundary.RequestID == nil || *boundary.RequestID != action.RequestID() {
		t.Errorf("last action %+v, want request %s", boundary, action.RequestID())
	}

	timeline, err := analysis.Timeline(ctx, termtrace.TimelineOptions{Limit: 10})
	fatal(t, err)

	if len(timeline.Entries) == 0 || len(timeline.Entries) > 10 {
		t.Errorf("timeline page has %d entries", len(timeline.Entries))
	}
}

func TestStructureVerificationAcceptsACompletedSessionAndRejectsATornLog(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo INTEGRITY_GO; sleep 30"},
		termtrace.LaunchOptions{},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("INTEGRITY_GO"), 5*time.Second)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	path := string(recording.Path())
	options := termtrace.VerifyOptions{Level: termtrace.VerificationLevelStructure}

	report, err := client.Integrity(path).Verify(ctx, options)
	fatal(t, err)

	if !report.Valid || report.Level != termtrace.VerificationLevelStructure ||
		len(report.Findings) != 0 {
		t.Errorf("verification of a completed session %+v", report)
	}

	file, err := os.OpenFile(filepath.Join(path, "events.jsonl"), os.O_APPEND|os.O_WRONLY, 0)
	fatal(t, err)
	_, err = file.WriteString(`{"torn":`)
	fatal(t, err)
	fatal(t, file.Close())

	report, err = client.Integrity(path).Verify(ctx, options)
	fatal(t, err)

	if report.Valid || len(report.Findings) == 0 {
		t.Errorf("verification of a torn log %+v", report)
	}
}

func TestAsciicastExportWritesThePlayback(t *testing.T) {
	client, dir := newClient(t)
	ctx := within(t, 30*time.Second)

	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "echo EXPORT_GO_OK; sleep 30"},
		termtrace.LaunchOptions{Rows: new(24), Cols: new(80)},
	)
	_, err := recording.Screen().WaitForText(ctx, "EXPORT_GO_OK", 5*time.Second)
	fatal(t, err)

	destination := filepath.Join(dir, "playback.cast")

	written, err := recording.Evidence().Asciicast(ctx, destination)
	fatal(t, err)

	if string(written) != destination {
		t.Errorf("exported to %q", written)
	}

	data, err := os.ReadFile(destination)
	fatal(t, err)

	header, _, _ := bytes.Cut(data, []byte("\n"))

	var playback struct {
		Version int `json:"version"`
	}

	fatal(t, json.Unmarshal(header, &playback, json.RejectUnknownMembers(false)))

	if playback.Version == 0 || !bytes.Contains(data, []byte("EXPORT_GO_OK")) {
		t.Errorf(
			"playback header %+v, contains output %v",
			playback,
			bytes.Contains(data, []byte("EXPORT_GO_OK")),
		)
	}
}

func TestCacheStatusReportsTheCheckpointACompletedObservationWrites(t *testing.T) {
	client, _ := newClient(t)
	ctx := within(t, 30*time.Second)
	recording := launch(
		t,
		client,
		[]string{"/bin/sh", "-c", "printf CACHED-SCREEN"},
		termtrace.LaunchOptions{},
	)
	_, err := recording.Output().WaitFor(ctx, []byte("CACHED-SCREEN"), 5*time.Second)
	fatal(t, err)
	fatal(t, recording.Stop(ctx))

	session, err := client.Attach(ctx, string(recording.Path()))
	fatal(t, err)

	analysis := session.Analysis()

	before, err := analysis.CacheStatus(ctx)
	fatal(t, err)

	if before.Entries != 0 || before.CompressedBytes != 0 {
		t.Errorf(
			"cache before observing = %d entries, %d bytes",
			before.Entries,
			before.CompressedBytes,
		)
	}

	first, err := session.Screen().Observe(ctx, termtrace.ObserveOptions{})
	fatal(t, err)

	if first.Resolution != termtrace.ObservationResolutionCanonicalReplay {
		t.Errorf("first observation resolution = %s", first.Resolution)
	}

	after, err := analysis.CacheStatus(ctx)
	fatal(t, err)

	if after.Entries != 1 || after.CompressedBytes == 0 {
		t.Errorf(
			"cache after observing = %d entries, %d bytes",
			after.Entries,
			after.CompressedBytes,
		)
	}

	if !strings.HasPrefix(string(after.Directory), string(recording.Path())) {
		t.Errorf("cache directory %s is outside the session %s", after.Directory, recording.Path())
	}

	second, err := session.Screen().Observe(ctx, termtrace.ObserveOptions{})
	fatal(t, err)

	if second.Resolution != termtrace.ObservationResolutionValidatedCheckpoint {
		t.Errorf("second observation resolution = %s", second.Resolution)
	}

	_, err = analysis.CachePrune(ctx)
	fatal(t, err)

	pruned, err := analysis.CacheStatus(ctx)
	fatal(t, err)

	if pruned.Entries != 0 {
		t.Errorf("cache after prune = %d entries", pruned.Entries)
	}
}
