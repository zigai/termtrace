package termtrace

import (
	"context"
	"time"
)

// TmuxOptions selects the tmux server and the state root of aggregate recordings.
//
//exhaustruct:optional
type TmuxOptions struct {
	// Root holds aggregate recordings; the default registry root when empty.
	Root string
	// Tmux is the tmux executable; tmux on PATH when empty.
	Tmux string
	// Socket selects the tmux server; the default socket when nil.
	Socket TmuxSocket
}

// Tmux manages aggregate recordings of a tmux server. Creating it starts nothing.
type Tmux struct {
	client *Client
	root   *Path //exhaustruct:optional
	tmux   *Path //exhaustruct:optional
	socket TmuxSocket
}

// TmuxStopOptions sets whether and how long a stop waits for the tracks.
//
//exhaustruct:optional
type TmuxStopOptions struct {
	// NoWait returns without waiting for every track to complete.
	NoWait bool
	// Timeout bounds waiting; the default wait timeout when zero.
	Timeout time.Duration
}

// TmuxPageOptions selects one page of aggregate history or catalog entries.
//
//exhaustruct:optional
type TmuxPageOptions struct {
	// FromSequence is the first sequence to return; 1 when zero.
	FromSequence uint64
	// Limit is the maximum number of entries; 100 when zero.
	Limit int
}

// TmuxCatalogOptions selects what a catalog page lists.
//
//exhaustruct:optional
type TmuxCatalogOptions struct {
	TmuxPageOptions

	// Gaps lists coverage gaps instead of tracks.
	Gaps bool
	// ActiveOnly lists only active tracks.
	ActiveOnly bool
}

// Tmux returns the aggregate recording manager for options.
func (c *Client) Tmux(options TmuxOptions) *Tmux {
	tmux := &Tmux{client: c, socket: options.Socket}
	if options.Root != "" {
		tmux.root = new(Path(options.Root))
	}

	if options.Tmux != "" {
		tmux.tmux = new(Path(options.Tmux))
	}

	return tmux
}

// Status reports the tmux server and its Termtrace integration.
func (t *Tmux) Status(ctx context.Context) (TmuxStatusReport, error) {
	return t.client.tmuxStatus(ctx, tmuxStatusParams{Root: t.root, Tmux: t.tmux, Socket: t.socket})
}

// Record starts an aggregate recording.
func (t *Tmux) Record(
	ctx context.Context,
	options TmuxRecordConfiguration,
) (TmuxRecordingStart, error) {
	return t.client.tmuxRecord(ctx, tmuxRecordParams{
		Root:    t.root,
		Tmux:    t.tmux,
		Socket:  t.socket,
		Options: options,
	})
}

// RecordingStatus reports the lifecycle of the aggregate recording name.
func (t *Tmux) RecordingStatus(ctx context.Context, name string) (TmuxRecordingManifest, error) {
	return t.client.tmuxRecordingStatus(ctx, tmuxNameParams{
		Root:   t.root,
		Tmux:   t.tmux,
		Socket: t.socket,
		Name:   name,
	})
}

// Track registers the session directory session as the track of pane, such as %3, in the
// aggregate recording whose manifest is at manifest.
func (t *Tmux) Track(ctx context.Context, manifest string, pane string, session string) error {
	return t.client.tmuxTrack(ctx, tmuxTrackParams{
		Manifest: Path(manifest),
		Pane:     pane,
		Session:  Path(session),
	})
}

// Stop stops the tracks of the aggregate recording name.
func (t *Tmux) Stop(
	ctx context.Context,
	name string,
	options TmuxStopOptions,
) (TmuxRecordingManifest, error) {
	params := tmuxStopParams{
		Root:      t.root,
		Tmux:      t.tmux,
		Socket:    t.socket,
		Name:      name,
		TimeoutMs: milliseconds(options.Timeout),
	}
	if options.NoWait {
		params.Wait = new(false)
	}

	return t.client.tmuxStop(ctx, params)
}

// Events reads one page of the history of the aggregate recording name.
func (t *Tmux) Events(ctx context.Context, name string, page TmuxPageOptions) (TmuxEvents, error) {
	return t.client.tmuxEvents(ctx, tmuxEventsParams{
		Root:         t.root,
		Tmux:         t.tmux,
		Socket:       t.socket,
		Name:         name,
		FromSequence: optional(page.FromSequence),
		Limit:        positive(page.Limit),
	})
}

// Catalog reads one page of the tracks or coverage gaps of the aggregate recording name.
func (t *Tmux) Catalog(
	ctx context.Context,
	name string,
	options TmuxCatalogOptions,
) (TmuxCatalog, error) {
	return t.client.tmuxCatalog(ctx, tmuxCatalogParams{
		Root:         t.root,
		Tmux:         t.tmux,
		Socket:       t.socket,
		Name:         name,
		Gaps:         options.Gaps,
		ActiveOnly:   options.ActiveOnly,
		FromSequence: optional(options.FromSequence),
		Limit:        positive(options.Limit),
	})
}

// Bundle writes the aggregate recording whose manifest is at manifest into the new bundle
// directory.
func (t *Tmux) Bundle(
	ctx context.Context,
	manifest string,
	bundle string,
) (TmuxStreamingBundleManifest, error) {
	return t.client.tmuxBundle(
		ctx,
		tmuxBundleParams{Manifest: Path(manifest), Bundle: Path(bundle)},
	)
}

// VerifyBundle verifies the aggregate bundle directory bundle against the manifest at manifest.
func (t *Tmux) VerifyBundle(
	ctx context.Context,
	manifest string,
	bundle string,
) (TmuxStreamingBundleManifest, error) {
	return t.client.tmuxVerifyBundle(
		ctx,
		tmuxBundleParams{Manifest: Path(manifest), Bundle: Path(bundle)},
	)
}
