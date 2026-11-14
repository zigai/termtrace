package consumer

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"syscall"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

const systemTmux = "/usr/bin/tmux"

func requireTmux(t *testing.T) {
	t.Helper()

	if _, err := os.Stat(systemTmux); err != nil {
		t.Skipf("tmux is not installed at %s", systemTmux)
	}
}

func tmuxCommand(ctx context.Context, socket string, args ...string) (string, error) {
	output, err := exec.CommandContext(ctx, systemTmux, append([]string{"-S", socket}, args...)...).
		Output()

	return string(output), err
}

func killTmuxServer(t *testing.T, socket string) {
	t.Helper()

	_, _ = tmuxCommand(context.Background(), socket, "kill-server")

	if _, err := tmuxCommand(context.Background(), socket, "list-sessions"); err == nil {
		t.Errorf("the private tmux server at %s still runs", socket)
	}
}

func tmuxSocket(socket string) termtrace.TmuxSocketPath {
	return termtrace.TmuxSocketPath{Path: termtrace.Base64Bytes(socket)}
}

func newSessionCommand(name string) []termtrace.Base64Bytes {
	return []termtrace.Base64Bytes{
		termtrace.Base64Bytes(systemTmux),
		termtrace.Base64Bytes("new-session"),
		termtrace.Base64Bytes("-s"),
		termtrace.Base64Bytes(name),
	}
}

// eventually retries operation until accept holds; a new private server creates its socket
// only after record returns, so early calls may fail.
func eventually[T any](
	t *testing.T,
	operation func() (T, error),
	accept func(T) bool,
) T {
	t.Helper()

	deadline := time.Now().Add(8 * time.Second)

	for {
		value, err := operation()
		if err == nil && accept(value) {
			return value
		}

		if time.Now().After(deadline) {
			t.Fatalf("private tmux condition did not become true: %v", err)
		}

		time.Sleep(25 * time.Millisecond)
	}
}

func stopQuietly(tmux *termtrace.Tmux, name string) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	_, _ = tmux.Stop(ctx, name, termtrace.TmuxStopOptions{Timeout: 10 * time.Second})
}

func TestTmuxPrivateSocketTrackLifecycle(t *testing.T) {
	requireTmux(t)

	root := t.TempDir()
	socket := filepath.Join(root, "private.sock")
	helper := filepath.Join(root, "selected ' helper")
	fatal(t, os.Symlink(binary(t), helper))

	client := clientWithBinary(t, helper)
	tmux := client.Tmux(termtrace.TmuxOptions{
		Root:   filepath.Join(root, "state ' quoted"),
		Tmux:   systemTmux,
		Socket: tmuxSocket(socket),
	})

	t.Cleanup(func() { killTmuxServer(t, socket) })

	report, err := tmux.Record(t.Context(), termtrace.TmuxRecordConfiguration{
		Name:           "native",
		NewServer:      true,
		GhosttyProfile: termtrace.TmuxGhosttyProfileOff,
		Command:        newSessionCommand("native"),
	})
	fatal(t, err)
	t.Cleanup(func() { stopQuietly(tmux, "native") })

	audit := eventually(t, func() (termtrace.TmuxStatusReport, error) {
		return tmux.Status(t.Context())
	}, func(status termtrace.TmuxStatusReport) bool { return status.RecordedPanes >= 1 })

	resolved, err := filepath.EvalSymlinks(binary(t))
	fatal(t, err)

	if string(audit.ConfiguredExecutable) != resolved {
		t.Fatalf("configured executable = %q, want %q", audit.ConfiguredExecutable, resolved)
	}

	if !audit.Enabled || !audit.DefaultCommandMatches {
		t.Fatalf(
			"enabled %v, default command matches %v",
			audit.Enabled,
			audit.DefaultCommandMatches,
		)
	}

	tracks := eventually(t, func() (termtrace.TmuxCatalogTracks, error) {
		catalog, err := tmux.Catalog(t.Context(), "native", termtrace.TmuxCatalogOptions{})
		if err != nil {
			return termtrace.TmuxCatalogTracks{}, fmt.Errorf("catalog: %w", err)
		}

		tracks, ok := catalog.(termtrace.TmuxCatalogTracks)
		if !ok {
			t.Fatalf("catalog = %T, want tracks", catalog)
		}

		return tracks, nil
	}, func(tracks termtrace.TmuxCatalogTracks) bool { return len(tracks.Entries) >= 1 })

	manifest := string(report.Manifest)
	entry := tracks.Entries[0].Track

	fatal(t, tmux.Track(t.Context(), manifest, entry.PaneID, string(entry.Session)))

	if err := tmux.Track(t.Context(), manifest, "%987654321", string(entry.Session)); err == nil {
		t.Fatal("tracking a missing pane succeeded")
	}

	if err := tmux.Track(
		t.Context(),
		manifest,
		entry.PaneID,
		filepath.Join(root, "missing-source"),
	); err == nil {
		t.Fatal("tracking a missing session succeeded")
	}

	catalog, err := tmux.Catalog(t.Context(), "native", termtrace.TmuxCatalogOptions{Gaps: true})
	fatal(t, err)

	gaps, ok := catalog.(termtrace.TmuxCatalogGaps)
	if !ok || len(gaps.Entries) == 0 {
		t.Fatalf("gap catalog = %#v, want at least one gap", catalog)
	}

	history, err := tmux.Events(
		t.Context(),
		"native",
		termtrace.TmuxPageOptions{FromSequence: 1, Limit: 1000},
	)
	fatal(t, err)

	if len(history.Page.Events) == 0 {
		t.Fatal("history has no events")
	}

	ended, err := tmux.Stop(
		t.Context(),
		"native",
		termtrace.TmuxStopOptions{Timeout: 10 * time.Second},
	)
	fatal(t, err)

	if ended.Lifecycle != termtrace.TmuxRecordingLifecycleCompleted {
		t.Fatalf("lifecycle = %s, want completed", ended.Lifecycle)
	}

	again, err := tmux.Stop(
		t.Context(),
		"native",
		termtrace.TmuxStopOptions{Timeout: 10 * time.Second},
	)
	fatal(t, err)

	if again.LifecycleJournal != ended.LifecycleJournal {
		t.Fatalf(
			"a repeated stop moved the journal from %+v to %+v",
			ended.LifecycleJournal,
			again.LifecycleJournal,
		)
	}

	catalog, err = tmux.Catalog(t.Context(), "native", termtrace.TmuxCatalogOptions{})
	fatal(t, err)

	closed, ok := catalog.(termtrace.TmuxCatalogTracks)
	if !ok || closed.Summary.UnfinishedTracks != 0 {
		t.Fatalf("closed catalog = %#v, want no unfinished tracks", catalog)
	}

	for _, entry := range closed.Entries {
		if !entry.Completed {
			t.Fatalf("track %s is not completed", entry.Track.PaneID)
		}
	}

	bundlePath := filepath.Join(root, "aggregate-bundle")

	bundle, err := tmux.Bundle(t.Context(), manifest, bundlePath)
	fatal(t, err)

	if bundle.TrackCount < 2 {
		t.Fatalf("bundle has %d tracks, want the composite and a pane", bundle.TrackCount)
	}

	verified, err := tmux.VerifyBundle(t.Context(), manifest, bundlePath)
	fatal(t, err)

	if !sameJSON(t, verified, bundle) {
		t.Fatalf("verified %+v, bundled %+v", verified, bundle)
	}

	if _, err := tmux.Bundle(t.Context(), manifest, bundlePath); err == nil {
		t.Fatal("bundling over an existing bundle succeeded")
	}

	mismatch := filepath.Join(root, "mismatch", "native")
	fatal(t, os.CopyFS(mismatch, os.DirFS(filepath.Dir(manifest))))

	mismatchFile := filepath.Join(mismatch, "manifest.json")
	original, err := os.ReadFile(mismatchFile)
	fatal(t, err)

	altered := regexp.MustCompile(`("recording_id"\s*:\s*")[^"]+"`).
		ReplaceAll(original, []byte(`${1}00000000-0000-0000-0000-000000000000"`))
	if bytes.Equal(altered, original) {
		t.Fatal("the manifest has no recording identity to alter")
	}

	fatal(t, os.WriteFile(mismatchFile, altered, 0o600))

	if _, err := tmux.VerifyBundle(t.Context(), mismatchFile, bundlePath); err == nil {
		t.Fatal("verifying against another recording's manifest succeeded")
	}
}

func TestTmuxAdmissionFailureRestoresOptions(t *testing.T) {
	requireTmux(t)

	root := t.TempDir()
	socket := filepath.Join(root, "rollback.sock")

	if _, err := tmuxCommand(
		t.Context(),
		socket,
		"-f",
		"/dev/null",
		"new-session",
		"-d",
		"-s",
		"external",
	); err != nil {
		t.Fatalf("start the external tmux server: %v", err)
	}

	t.Cleanup(func() { killTmuxServer(t, socket) })

	original, err := tmuxCommand(t.Context(), socket, "show-option", "-gv", "default-command")
	fatal(t, err)

	tmux := clientWithBinary(t, failingBinary(t)).Tmux(termtrace.TmuxOptions{
		Root:   filepath.Join(root, "state"),
		Tmux:   systemTmux,
		Socket: tmuxSocket(socket),
	})

	_, err = tmux.Record(t.Context(), termtrace.TmuxRecordConfiguration{
		Name:           "failed",
		GhosttyProfile: termtrace.TmuxGhosttyProfileOff,
		Command: []termtrace.Base64Bytes{
			termtrace.Base64Bytes(systemTmux),
			termtrace.Base64Bytes("attach-session"),
			termtrace.Base64Bytes("-t"),
			termtrace.Base64Bytes("external"),
		},
	})
	if err == nil {
		t.Fatal("recording with a failing helper succeeded")
	}

	restored, err := tmuxCommand(t.Context(), socket, "show-option", "-gv", "default-command")
	fatal(t, err)

	if restored != original {
		t.Fatalf("default-command = %q, want %q", restored, original)
	}

	version, err := tmuxCommand(
		t.Context(),
		socket,
		"show-option",
		"-gqv",
		"@termtrace-recording-version",
	)
	fatal(t, err)

	if version != "" && version != "\n" {
		t.Fatalf("integration version option left behind: %q", version)
	}

	if _, err := os.Stat(
		filepath.Join(root, "state", "tmux-recordings", "failed"),
	); !errors.Is(
		err,
		os.ErrNotExist,
	) {
		t.Fatalf("the failed recording left state behind: %v", err)
	}

	if _, err := tmuxCommand(t.Context(), socket, "has-session", "-t", "external"); err != nil {
		t.Fatalf("the external session is gone: %v", err)
	}
}

func TestTmuxNonUTF8PathsAndStopTimeoutFacts(t *testing.T) {
	requireTmux(t)

	root := t.TempDir()
	socket := filepath.Join(root, "s")
	state := root + "/state-\xff"

	client := clientWithBinary(t, binary(t))
	tmux := client.Tmux(
		termtrace.TmuxOptions{Root: state, Tmux: systemTmux, Socket: tmuxSocket(socket)},
	)

	t.Cleanup(func() { killTmuxServer(t, socket) })

	start, err := tmux.Record(t.Context(), termtrace.TmuxRecordConfiguration{
		Name:           "partial",
		NewServer:      true,
		GhosttyProfile: termtrace.TmuxGhosttyProfileOff,
		Command:        newSessionCommand("native"),
	})
	fatal(t, err)
	t.Cleanup(func() { stopQuietly(tmux, "partial") })

	eventually(t, func() (termtrace.TmuxStatusReport, error) {
		return tmux.Status(t.Context())
	}, func(status termtrace.TmuxStatusReport) bool { return status.RecordedPanes >= 1 })

	manifest := eventually(t, func() (termtrace.TmuxRecordingManifest, error) {
		return tmux.RecordingStatus(t.Context(), "partial")
	}, func(manifest termtrace.TmuxRecordingManifest) bool { return len(manifest.PaneTracks) > 0 })

	for _, tracks := range []map[string]termtrace.Base64Bytes{manifest.PaneTracks, start.Recording.PaneTracks} {
		for pane, path := range tracks {
			if !bytes.Contains(path, []byte{0xff}) {
				t.Fatalf("pane %s track path %q lost the non-UTF-8 byte", pane, path)
			}
		}
	}

	catalog, err := tmux.Catalog(t.Context(), "partial", termtrace.TmuxCatalogOptions{Gaps: true})
	fatal(t, err)

	gaps, ok := catalog.(termtrace.TmuxCatalogGaps)
	if !ok {
		t.Fatalf("gap catalog = %T", catalog)
	}

	unresolved := false
	for _, entry := range gaps.Entries {
		unresolved = unresolved || entry.Gap.ResolvedUnixNs == nil
	}

	if !unresolved {
		t.Fatalf("gaps = %+v, want an unresolved gap", gaps.Entries)
	}

	entry, err := client.Registry(state).Resolve(t.Context(), "tmux-partial-composite")
	fatal(t, err)

	if entry == nil || entry.RecorderPID == nil {
		t.Fatalf("composite registry entry = %+v, want a recorder PID", entry)
	}

	paused := *entry.RecorderPID
	fatal(t, syscall.Kill(paused, syscall.SIGSTOP))
	t.Cleanup(func() { _ = syscall.Kill(paused, syscall.SIGCONT) })

	_, err = tmux.Stop(
		t.Context(),
		"partial",
		termtrace.TmuxStopOptions{Timeout: 30 * time.Millisecond},
	)
	if err == nil {
		t.Fatal("stopping a paused recorder succeeded")
	}

	data := termtraceError(t, err).Facts
	if data == nil {
		t.Fatalf("stop error has no facts: %v", err)
	}

	if data.Scope != termtrace.FailureScopeManagement || data.Effect != nil || data.Retry != nil ||
		data.Guidance == nil || *data.Guidance != termtrace.ManagementGuidanceInspectNoAutomaticRetry ||
		data.Kind != "control_timeout" {
		t.Fatalf("stop error facts = %+v", data)
	}

	management, ok := data.Management.(termtrace.ManagementFailureTmux)
	if !ok {
		t.Fatalf("management facts = %T, want tmux", data.Management)
	}

	facts := management.Facts
	if facts.StopRequested == nil || !*facts.StopRequested ||
		facts.Phase != termtrace.TmuxFailurePhaseStopRequested ||
		!bytes.Equal(facts.Manifest, start.Manifest) ||
		len(facts.CompletedTrackStops) != 0 {
		t.Fatalf("tmux facts = %+v", facts)
	}
}
