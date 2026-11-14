package termtrace

import (
	"context"
	"time"
	"uuid"
)

// Session is a recorded terminal session, live or completed.
type Session struct {
	client    *Client
	id        string
	path      Path
	recording *string //exhaustruct:optional
}

// MutationOptions sets the identity and presented-state guard of one mutation.
type MutationOptions struct {
	// RequestID identifies the mutation so a retry is applied once; allocated when empty.
	RequestID string
	// IfState applies the mutation only while the presented state has this hash.
	IfState string
}

func (o MutationOptions) wire() mutation {
	return mutation{RequestID: optional(o.RequestID), IfState: optional(o.IfState)}
}

// Launch starts command in a new recorded terminal. The returned recording owns the session
// until it stops, detaches, or is closed.
func (c *Client) Launch(
	ctx context.Context,
	command []string,
	options LaunchOptions,
) (*Recording, error) {
	info, err := c.termtraceLaunch(ctx, launchParams{Command: command, Options: options})
	if err != nil {
		return nil, err
	}

	return &Recording{
		Session: &Session{
			client:    c,
			id:        info.SessionID,
			path:      info.Path,
			recording: new(info.Recording),
		},
		handle: info.Recording,
	}, nil
}

// Attach opens an existing session directory.
func (c *Client) Attach(ctx context.Context, path string) (*Session, error) {
	info, err := c.termtraceAttach(ctx, attachParams{Path: Path(path)})
	if err != nil {
		return nil, err
	}

	return &Session{client: c, id: info.SessionID, path: info.Path}, nil
}

// AttachTarget opens a session by registered name, session id, or path.
func (c *Client) AttachTarget(ctx context.Context, target string) (*Session, error) {
	info, err := c.termtraceAttachTarget(ctx, attachTargetParams{Target: target})
	if err != nil {
		return nil, err
	}

	return &Session{client: c, id: info.SessionID, path: info.Path}, nil
}

// ID returns the canonical session identity.
func (s *Session) ID() string {
	return s.id
}

// Path returns the session directory.
func (s *Session) Path() Path {
	return s.path
}

// Status returns live status, or the completion of a finished session. It also works after
// the owning recording stops.
func (s *Session) Status(ctx context.Context) (SessionStatus, error) {
	return s.client.sessionStatus(ctx, statusParams{Session: sessionRef{Path: s.path}})
}

// Send applies ordered input and control steps, then checks the optional postcondition.
func (s *Session) Send(ctx context.Context, spec SendSpec) (SendResult, error) {
	return s.client.sessionSend(ctx, sendParams{Session: s.ref(), Spec: spec})
}

// Resize changes the terminal to cols by rows cells.
func (s *Session) Resize(
	ctx context.Context,
	cols int,
	rows int,
	options MutationOptions,
) (ControlAcknowledgement, error) {
	return s.client.sessionResize(
		ctx,
		resizeParams{Session: s.ref(), Cols: cols, Rows: rows, Mutation: options.wire()},
	)
}

// Signal sends signal to the child process group.
func (s *Session) Signal(
	ctx context.Context,
	signal ProcessSignal,
	options MutationOptions,
) (ControlAcknowledgement, error) {
	return s.client.sessionSignal(
		ctx,
		signalParams{Session: s.ref(), Signal: signal, Mutation: options.wire()},
	)
}

// Mark appends a named marker to the event stream.
func (s *Session) Mark(
	ctx context.Context,
	label string,
	options MutationOptions,
) (MarkOutcome, error) {
	return s.client.sessionMark(
		ctx,
		markParams{Session: s.ref(), Label: label, Mutation: options.wire()},
	)
}

// Screen returns the screen of this session.
func (s *Session) Screen() *Screen {
	return &Screen{session: s}
}

// Pointer returns mouse, focus, and text-activation input for this session.
func (s *Session) Pointer() *Pointer {
	return &Pointer{session: s}
}

// Actions starts an input action with a fresh request identity.
func (s *Session) Actions() *ActionBuilder {
	return &ActionBuilder{session: s, spec: actionSpec{Atoms: nil, RequestID: newRequestID()}}
}

func (s *Session) ref() sessionRef {
	return sessionRef{Path: s.path, Recording: s.recording}
}

func optional[T comparable](value T) *T {
	var zero T
	if value == zero {
		return nil
	}

	return &value
}

func milliseconds(duration time.Duration) *uint64 {
	if duration <= 0 {
		return nil
	}

	return new(uint64(duration / time.Millisecond))
}

func newRequestID() string {
	return uuid.NewV7().String()
}
