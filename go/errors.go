package termtrace

import (
	"errors"
	"fmt"

	"github.com/zigai/termtrace/go/internal/rpc"
)

// Error is a failure the server reported.
type Error struct {
	// Message describes the failure.
	Message string
	// Facts are the typed failure facts: kind, code, mutation effect, retry rule, and details.
	// They are nil for a failure that carries none, such as a call on a stopped recording.
	Facts *ErrorData //exhaustruct:optional
}

func (e *Error) Error() string {
	return "termtrace: " + e.Message
}

// Kind returns the failure kind, such as "invalid_operation", "wait_unsatisfied", or
// "no_match", or "" when the failure has no typed facts.
func (e *Error) Kind() string {
	if e.Facts == nil {
		return ""
	}

	return e.Facts.Kind
}

func failure(err error) error {
	var response *rpc.Error
	if !errors.As(err, &response) {
		return err
	}

	converted := &Error{Message: response.Message}
	if len(response.Data) > 0 {
		var facts ErrorData
		if decodeErr := DecodeJSON(response.Data, &facts); decodeErr != nil {
			return fmt.Errorf(
				"termtrace: %s (undecodable error data: %w)",
				response.Message,
				decodeErr,
			)
		}

		converted.Facts = &facts
	}

	return converted
}
