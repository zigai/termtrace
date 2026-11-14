package termtrace

import (
	"context"
	"time"
)

// Recording is a session this client launched and owns. Its [Session] methods fail once the
// recording stops or detaches.
type Recording struct {
	*Session

	handle string
}

// StopOptions sets the identity of a stop request and how long to wait for completion.
type StopOptions struct {
	// RequestID identifies the stop so a retry is applied once; allocated when empty.
	RequestID string
	// Wait for validated completion this long; when zero, return once the stop is accepted.
	Wait time.Duration
}

// IsActive reports whether the recording is still running and owned by this client.
func (r *Recording) IsActive(ctx context.Context) (bool, error) {
	return r.client.recordingIsActive(ctx, recordingParams{Recording: r.handle})
}

// Stop stops the recording and waits for validated completion, up to the default stop timeout.
// Use [Recording.RequestStop] to choose the wait.
func (r *Recording) Stop(ctx context.Context) error {
	return r.client.recordingStop(ctx, timedRecordingParams{Recording: r.handle})
}

// RequestStop asks the recording to end. A failed request leaves the recording attached; retry
// it with the same [StopOptions.RequestID].
func (r *Recording) RequestStop(ctx context.Context, options StopOptions) (StopReport, error) {
	return r.client.recordingRequestStop(ctx, requestStopParams{
		Recording: r.handle,
		RequestID: optional(options.RequestID),
		WaitMs:    milliseconds(options.Wait),
	})
}

// WaitForCompletion waits up to timeout, or the default stop timeout when zero, for validated
// completion evidence and reaps the recorder.
func (r *Recording) WaitForCompletion(
	ctx context.Context,
	timeout time.Duration,
) (SessionCompletion, error) {
	return r.client.recordingWaitForCompletion(
		ctx,
		timedRecordingParams{Recording: r.handle, TimeoutMs: milliseconds(timeout)},
	)
}

// DiscardCompleted deletes this completed session, reaping the recorder.
func (r *Recording) DiscardCompleted(ctx context.Context) error {
	return r.client.recordingDiscardCompleted(ctx, recordingParams{Recording: r.handle})
}

// ForceClose terminates and reaps the recorded process group and the recorder.
func (r *Recording) ForceClose(ctx context.Context) error {
	return r.client.recordingForceClose(ctx, recordingParams{Recording: r.handle})
}

// Detach releases ownership so the recording keeps running in the background.
func (r *Recording) Detach(ctx context.Context) error {
	return r.client.recordingDetach(ctx, recordingParams{Recording: r.handle})
}

// Close releases the recording handle on the server. An active recording keeps running, as
// after [Recording.Detach].
func (r *Recording) Close(ctx context.Context) error {
	return r.client.recordingClose(ctx, recordingParams{Recording: r.handle})
}
