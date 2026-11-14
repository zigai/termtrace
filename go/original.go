package termtrace

import (
	"context"
	"time"
)

// OriginalSession is an original recording this client owns: a command running in Ghostty,
// captured from its first frame.
type OriginalSession struct {
	client *Client
	handle string
	id     string
	path   Path
}

// FinishOptions bounds finalization and identifies the stop request.
//
//exhaustruct:optional
type FinishOptions struct {
	// Timeout bounds finalization; the default stop timeout when zero.
	Timeout time.Duration
	// RequestID identifies the stop so a retry is applied once; allocated when empty.
	RequestID string
}

// RecordOriginal launches command in Ghostty and captures it from its first frame. Canceling
// ctx during startup tears the capture down.
func (c *Client) RecordOriginal(
	ctx context.Context,
	command []string,
	options OriginalLaunchOptions,
) (*OriginalSession, error) {
	info, err := c.termtraceRecordOriginal(
		ctx,
		recordOriginalParams{Command: command, Options: options},
	)
	if err != nil {
		return nil, err
	}

	return c.original(info), nil
}

// AttachOriginal owns the existing original recording at path, without its capture supervisor.
func (c *Client) AttachOriginal(ctx context.Context, path string) (*OriginalSession, error) {
	info, err := c.termtraceAttachOriginal(ctx, attachOriginalParams{Path: Path(path)})
	if err != nil {
		return nil, err
	}

	return c.original(info), nil
}

func (c *Client) original(info OriginalInfo) *OriginalSession {
	return &OriginalSession{client: c, handle: info.Original, id: info.SessionID, path: info.Path}
}

// ID returns the canonical session identity.
func (o *OriginalSession) ID() string {
	return o.id
}

// Path returns the session directory.
func (o *OriginalSession) Path() Path {
	return o.path
}

// Session returns live control and observation of the recording; a resize moves the captured
// window.
func (o *OriginalSession) Session() *Session {
	return &Session{client: o.client, id: o.id, path: o.path}
}

// IsActive reports whether this client still owns the recording.
func (o *OriginalSession) IsActive(ctx context.Context) (bool, error) {
	return o.client.originalIsActive(ctx, originalParams{Original: o.handle})
}

// CaptureStatus reports the capture supervisor status, abandoned when the supervisor is gone.
func (o *OriginalSession) CaptureStatus(ctx context.Context) (OriginalCaptureStatus, error) {
	return o.client.originalCaptureStatus(ctx, originalParams{Original: o.handle})
}

// Finish stops the program and waits until the capture is published. A finalization timeout
// keeps the recording owned for another wait or [OriginalSession.ForceClose].
func (o *OriginalSession) Finish(
	ctx context.Context,
	options FinishOptions,
) (OriginalRecording, error) {
	return o.client.originalFinish(ctx, finishParams{
		Original:  o.handle,
		RequestID: optional(options.RequestID),
		TimeoutMs: milliseconds(options.Timeout),
	})
}

// WaitFinished waits up to timeout, or the default stop timeout when zero, until the program
// exits and the capture is published, without stopping it.
func (o *OriginalSession) WaitFinished(
	ctx context.Context,
	timeout time.Duration,
) (OriginalRecording, error) {
	return o.client.originalWaitFinished(
		ctx,
		waitFinishedParams{Original: o.handle, TimeoutMs: milliseconds(timeout)},
	)
}

// ForceClose kills the program's process group and the capture supervisor; the capture is
// abandoned. It does nothing once the recording finished or detached.
func (o *OriginalSession) ForceClose(ctx context.Context) error {
	return o.client.originalForceClose(ctx, originalParams{Original: o.handle})
}

// Detach leaves the recording running and gives up ownership.
func (o *OriginalSession) Detach(ctx context.Context) error {
	return o.client.originalDetach(ctx, originalParams{Original: o.handle})
}

// Close releases the handle on the server.
func (o *OriginalSession) Close(ctx context.Context) error {
	return o.client.originalClose(ctx, originalParams{Original: o.handle})
}
