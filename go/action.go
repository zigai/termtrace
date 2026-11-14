package termtrace

import (
	"context"
	"fmt"
	"time"
)

// ActionBuilder composes one input action: atoms, pacing, a guard, and a postcondition. Run
// seals it, so running it again retries the same request instead of typing twice. Changing a
// sealed action leaves the frozen request unchanged and fails the next Run with kind
// "invalid_operation".
type ActionBuilder struct {
	session *Session
	spec    actionSpec
	sealed  bool  //exhaustruct:optional
	err     error //exhaustruct:optional
}

// IdleOptions bounds an idle postcondition.
type IdleOptions struct {
	// StableFor is how long the screen must stay unchanged; the default idle interval when zero.
	StableFor time.Duration
	// Timeout bounds the wait; the default wait timeout when zero.
	Timeout time.Duration
	// ReactionTimeout bounds the wait for the screen to react; the default when zero.
	ReactionTimeout time.Duration
}

// ActionResult is the outcome of one input action.
type ActionResult struct {
	ActOutcome
}

// RequestID returns the identity this action is sent with.
func (b *ActionBuilder) RequestID() string {
	return b.spec.RequestID
}

// Sealed reports whether the action has run or been sealed.
func (b *ActionBuilder) Sealed() bool {
	return b.sealed
}

// WithRequestID sets the identity, a hyphenated non-nil UUID, to retry an earlier action.
func (b *ActionBuilder) WithRequestID(requestID string) *ActionBuilder {
	return b.edit(
		"change the request identity of",
		func(spec *actionSpec) { spec.RequestID = requestID },
	)
}

// IfState applies the action only while the presented state has stateHash.
func (b *ActionBuilder) IfState(stateHash string) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) { spec.IfState = new(stateHash) })
}

// SendText types text.
func (b *ActionBuilder) SendText(text string) *ActionBuilder {
	return b.edit(
		"change",
		func(spec *actionSpec) { spec.Atoms = append(spec.Atoms, AtomSpecText{Text: text}) },
	)
}

// SendKey presses one key chord, such as "Enter", "ctrl+c", or "ctrl+shift+F5".
func (b *ActionBuilder) SendKey(key string) *ActionBuilder {
	return b.edit(
		"change",
		func(spec *actionSpec) { spec.Atoms = append(spec.Atoms, AtomSpecKey{Key: key}) },
	)
}

// SendBytes writes raw bytes.
func (b *ActionBuilder) SendBytes(data []byte) *ActionBuilder {
	return b.edit(
		"change",
		func(spec *actionSpec) { spec.Atoms = append(spec.Atoms, AtomSpecBytes{Bytes: data}) },
	)
}

// Paste pastes text, bracketed when the application enabled bracketed paste.
func (b *ActionBuilder) Paste(text string) *ActionBuilder {
	return b.edit(
		"change",
		func(spec *actionSpec) { spec.Atoms = append(spec.Atoms, AtomSpecPaste{Paste: text}) },
	)
}

// Pace waits delay between atoms.
func (b *ActionBuilder) Pace(delay time.Duration) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) { spec.PaceMs = milliseconds(delay) })
}

// Intent records a free-form description with the input.
func (b *ActionBuilder) Intent(intent string) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) { spec.Intent = new(intent) })
}

// Verbose reports the outcome of every atom.
func (b *ActionBuilder) Verbose(enabled bool) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) { spec.Verbose = enabled })
}

// AllowIncomplete returns an unsettled postcondition instead of failing.
func (b *ActionBuilder) AllowIncomplete(enabled bool) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) { spec.AllowIncomplete = enabled })
}

// Projection selects the detail and region of the returned frame.
func (b *ActionBuilder) Projection(projection Projection) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) { spec.Projection = projection.wire() })
}

// UntilScreen waits after the input until the screen contains text.
func (b *ActionBuilder) UntilScreen(text string, timeout time.Duration) *ActionBuilder {
	return b.UntilScreenWhere(ScreenPredicateContains{Text: text}, timeout)
}

// UntilScreenWhere waits after the input until predicate holds.
func (b *ActionBuilder) UntilScreenWhere(
	predicate ScreenPredicate,
	timeout time.Duration,
) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) {
		spec.Until = ConditionSpecScreen{Predicate: predicate, TimeoutMs: milliseconds(timeout)}
	})
}

// UntilIdle waits after the input until the screen reacts and then stays unchanged.
func (b *ActionBuilder) UntilIdle(options IdleOptions) *ActionBuilder {
	return b.edit("change", func(spec *actionSpec) {
		var reaction *ReactionSpec
		if options.ReactionTimeout > 0 {
			reaction = &ReactionSpec{TimeoutMs: milliseconds(options.ReactionTimeout)}
		}

		spec.Until = ConditionSpecIdle{
			Reaction:    reaction,
			StableForMs: milliseconds(options.StableFor),
			TimeoutMs:   milliseconds(options.Timeout),
		}
	})
}

// Seal freezes the action without running it.
func (b *ActionBuilder) Seal() {
	b.sealed = true
}

// Run seals the action, applies it, and checks its postcondition. Running a sealed action
// again retries the same request; the server returns the first outcome instead of typing
// twice.
func (b *ActionBuilder) Run(ctx context.Context) (ActionResult, error) {
	b.sealed = true
	if err := b.err; err != nil {
		b.err = nil
		return ActionResult{}, err
	}

	outcome, err := b.session.client.actionRun(
		ctx,
		runParams{Session: b.session.ref(), Action: b.spec},
	)

	return ActionResult{ActOutcome: outcome}, err
}

func (b *ActionBuilder) edit(change string, apply func(*actionSpec)) *ActionBuilder {
	if b.sealed {
		b.err = &Error{
			Message: fmt.Sprintf(
				"cannot %s a sealed action; Run retries the frozen request %s",
				change,
				b.spec.RequestID,
			),
			Facts: &ErrorData{
				Kind:   "invalid_operation",
				Code:   "invalid_operation",
				Scope:  FailureScopeActor,
				Effect: MutationEffectNotApplied{},
				Retry:  new(RetryNewRequest),
			},
		}

		return b
	}

	apply(&b.spec)

	return b
}

// Matched reports whether the postcondition held.
func (r ActionResult) Matched() bool {
	switch postcondition := r.Postcondition.(type) {
	case ActionPostconditionResultNone:
		return postcondition.Matched
	case ActionPostconditionResultIdle:
		return postcondition.Matched
	case ActionPostconditionResultScreen:
		return postcondition.Matched
	default:
		return false
	}
}

// Frame returns the frame the postcondition observed, or nil when none was projected.
func (r ActionResult) Frame() *ScreenFrame {
	switch postcondition := r.Postcondition.(type) {
	case ActionPostconditionResultNone:
		return postcondition.Frame
	case ActionPostconditionResultIdle:
		return postcondition.Frame
	case ActionPostconditionResultScreen:
		return postcondition.Frame
	default:
		return nil
	}
}

// StateHash returns the state hash the postcondition observed, or "" when unknown.
func (r ActionResult) StateHash() string {
	var hash *string

	switch postcondition := r.Postcondition.(type) {
	case ActionPostconditionResultNone:
		hash = postcondition.StateHash
	case ActionPostconditionResultIdle:
		hash = postcondition.StateHash
	case ActionPostconditionResultScreen:
		hash = postcondition.StateHash
	}

	if hash != nil {
		return *hash
	}

	if frame := r.Frame(); frame != nil {
		return frame.StateHash
	}

	return ""
}
