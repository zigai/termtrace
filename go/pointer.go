package termtrace

import "context"

// Pointer sends mouse, focus, and text-activation input in the application's reporting mode.
type Pointer struct {
	session *Session
}

// PointerOptions configures a click.
type PointerOptions struct {
	MutationOptions

	// Button is the mouse button; left when empty.
	Button MouseButton
	// Strict fails instead of doing nothing when the application does not report mouse input.
	Strict bool
}

// DragOptions configures a drag.
type DragOptions struct {
	MutationOptions

	// Button is the mouse button; left when empty.
	Button MouseButton
	// Steps is the number of motion samples including the destination; 1 when zero.
	Steps int
	// Strict fails instead of doing nothing when the application does not report mouse input.
	Strict bool
}

// ScrollOptions configures a wheel scroll.
type ScrollOptions struct {
	MutationOptions

	// Lines is the signed number of wheel ticks; negative scrolls up; 1 when zero.
	Lines int
	// Strict fails instead of doing nothing when the application does not report mouse input.
	Strict bool
}

// FocusOptions configures a focus change.
type FocusOptions struct {
	MutationOptions

	// Strict fails instead of doing nothing when the application does not report focus.
	Strict bool
}

// ActivateOptions configures a text activation.
type ActivateOptions struct {
	MutationOptions

	// Unique requires exactly one match.
	Unique bool
	// Strict fails instead of doing nothing when the application does not report mouse input.
	Strict bool
}

func button(button MouseButton) MouseButton {
	if button == "" {
		return MouseButtonLeft
	}

	return button
}

// Click clicks the cell at column and row.
func (p *Pointer) Click(
	ctx context.Context,
	column int,
	row int,
	options PointerOptions,
) (PointerOutcome, error) {
	return p.apply(
		ctx,
		column,
		row,
		PointerOperationClick{Button: button(options.Button)},
		options.Strict,
		options.MutationOptions,
	)
}

// MultiClick clicks the cell count times in a row, such as 2 for a double click.
func (p *Pointer) MultiClick(
	ctx context.Context,
	column int,
	row int,
	count int,
	options PointerOptions,
) (PointerOutcome, error) {
	return p.apply(
		ctx,
		column,
		row,
		PointerOperationMultiClick{Button: button(options.Button), Count: count},
		options.Strict,
		options.MutationOptions,
	)
}

// Drag presses at column and row, moves to toColumn and toRow, and releases.
func (p *Pointer) Drag(
	ctx context.Context,
	column int,
	row int,
	toColumn int,
	toRow int,
	options DragOptions,
) (PointerOutcome, error) {
	steps := options.Steps
	if steps == 0 {
		steps = 1
	}

	operation := PointerOperationDrag{
		Button:   button(options.Button),
		ToColumn: toColumn,
		ToRow:    toRow,
		Steps:    steps,
	}

	return p.apply(ctx, column, row, operation, options.Strict, options.MutationOptions)
}

// Scroll turns the wheel at column and row.
func (p *Pointer) Scroll(
	ctx context.Context,
	column int,
	row int,
	options ScrollOptions,
) (PointerOutcome, error) {
	lines := options.Lines
	if lines == 0 {
		lines = 1
	}

	return p.apply(
		ctx,
		column,
		row,
		PointerOperationScroll{Lines: lines},
		options.Strict,
		options.MutationOptions,
	)
}

// Focus reports a terminal focus change to the application.
func (p *Pointer) Focus(
	ctx context.Context,
	focused bool,
	options FocusOptions,
) (PointerOutcome, error) {
	return p.session.client.sessionFocus(ctx, focusParams{
		Session:  p.session.ref(),
		Focused:  focused,
		Strict:   options.Strict,
		Mutation: options.wire(),
	})
}

// Activate clicks the visible text.
func (p *Pointer) Activate(
	ctx context.Context,
	text string,
	options ActivateOptions,
) (PointerOutcome, error) {
	return p.session.client.sessionActivate(ctx, activateParams{
		Session:  p.session.ref(),
		Text:     text,
		Unique:   options.Unique,
		Strict:   options.Strict,
		Mutation: options.wire(),
	})
}

func (p *Pointer) apply(
	ctx context.Context,
	column int,
	row int,
	operation PointerOperation,
	strict bool,
	options MutationOptions,
) (PointerOutcome, error) {
	return p.session.client.sessionPointer(ctx, pointerParams{
		Session:   p.session.ref(),
		Column:    column,
		Row:       row,
		Operation: operation,
		Strict:    strict,
		Mutation:  options.wire(),
	})
}
