package termtrace

import (
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
)

// Instrumentation emits application events and spans into a live session. Spans end only when
// the caller ends them.
type Instrumentation struct {
	client            *Client
	handle            string
	processInstanceID string
}

// Instrumentation opens an emitter bound to the session.
func (s *Session) Instrumentation(ctx context.Context) (*Instrumentation, error) {
	return s.client.Instrumentation(ctx, string(s.path))
}

// Instrumentation opens an emitter bound to the live session directory at path, such as the
// value of TERMTRACE_SESSION in a recorded program.
func (c *Client) Instrumentation(ctx context.Context, path string) (*Instrumentation, error) {
	info, err := c.instrumentationOpen(ctx, openParams{Session: Path(path)})
	if err != nil {
		return nil, err
	}

	return &Instrumentation{
		client:            c,
		handle:            info.Emitter,
		processInstanceID: info.ProcessInstanceID,
	}, nil
}

// ProcessInstanceID returns the process identity of every event and span of this emitter.
func (i *Instrumentation) ProcessInstanceID() string {
	return i.processInstanceID
}

// Event emits one event and returns the recorder's acknowledgement.
func (i *Instrumentation) Event(
	ctx context.Context,
	name string,
	attributes map[string]any,
) (ApplicationAcknowledgement, error) {
	encoded, err := encodeAttributes(attributes)
	if err != nil {
		return ApplicationAcknowledgement{}, err
	}

	return i.client.instrumentationEvent(
		ctx,
		eventParams{Emitter: i.handle, Name: name, Attributes: encoded},
	)
}

// StartSpan starts a span inside parentSpanID, or at the top level when it is empty.
func (i *Instrumentation) StartSpan(
	ctx context.Context,
	name string,
	parentSpanID string,
	attributes map[string]any,
) (ApplicationAcknowledgement, error) {
	encoded, err := encodeAttributes(attributes)
	if err != nil {
		return ApplicationAcknowledgement{}, err
	}

	return i.client.instrumentationStartSpan(ctx, startSpanParams{
		Emitter:      i.handle,
		Name:         name,
		ParentSpanID: optional(parentSpanID),
		Attributes:   encoded,
	})
}

// EndSpan ends the span spanID.
func (i *Instrumentation) EndSpan(
	ctx context.Context,
	name string,
	spanID string,
	attributes map[string]any,
) (ApplicationAcknowledgement, error) {
	encoded, err := encodeAttributes(attributes)
	if err != nil {
		return ApplicationAcknowledgement{}, err
	}

	return i.client.instrumentationEndSpan(ctx, endSpanParams{
		Emitter:    i.handle,
		Name:       name,
		SpanID:     spanID,
		Attributes: encoded,
	})
}

// Close releases the emitter handle; open spans stay open.
func (i *Instrumentation) Close(ctx context.Context) error {
	return i.client.instrumentationClose(ctx, closeParams{Emitter: i.handle})
}

func encodeAttributes(attributes map[string]any) (map[string]jsontext.Value, error) {
	encoded := make(map[string]jsontext.Value, len(attributes))
	for key, value := range attributes {
		data, err := json.Marshal(value)
		if err != nil {
			return nil, fmt.Errorf("encode attribute %q: %w", key, err)
		}

		encoded[key] = data
	}

	return encoded, nil
}
