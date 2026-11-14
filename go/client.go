// Package termtrace records terminal programs and drives them: launch a command in a recorded
// terminal, send input, wait for the screen, and inspect the evidence.
//
// Waits take explicit timeouts, so a timeout is reported as a typed [Error]. Canceling the
// context abandons a call and asks the server to cancel it; locator waits and capability probes
// stop at once, other calls finish on the server and their result is dropped.
//
// The package runs `termtrace sdk-server` as a child process and talks to it over stdin and
// stdout. The termtrace CLI must be installed at the same version as this module.
package termtrace

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"

	"github.com/zigai/termtrace/go/internal/rpc"
)

var errVersionMismatch = errors.New("termtrace: version mismatch")

// Options configures a [Client].
type Options struct {
	// Binary is the termtrace CLI; $TERMTRACE_BIN, then termtrace on PATH, by default.
	Binary string
	// EvidenceRoot holds new sessions; $TERMTRACE_ROOT, then termtrace in the temporary
	// directory, by default.
	EvidenceRoot string
	// Stderr receives the server's diagnostics; os.Stderr by default.
	Stderr io.Writer
}

// Client is a connection to one termtrace SDK server. It is safe for concurrent use.
type Client struct {
	conn *rpc.Conn
}

// New starts the SDK server and checks that its version matches [Version].
func New(ctx context.Context, options Options) (*Client, error) {
	binary, err := resolveBinary(options.Binary)
	if err != nil {
		return nil, err
	}

	stderr := options.Stderr
	if stderr == nil {
		stderr = os.Stderr
	}

	conn, err := rpc.Start(ctx, binary, []string{"sdk-server"}, stderr)
	if err != nil {
		return nil, err //nolint:wrapcheck // rpc errors are already termtrace errors.
	}

	client := &Client{conn: conn}

	params := initializeParams{
		Binary:       new(Path(binary)),
		EvidenceRoot: optional(Path(options.EvidenceRoot)),
	}

	info, err := client.initialize(ctx, params)
	if err != nil {
		return nil, errors.Join(err, conn.Close())
	}

	if info.Version != Version {
		return nil, errors.Join(fmt.Errorf(
			"%w: CLI %s at %s does not match Go SDK %s; install the matching CLI with "+
				"https://github.com/zigai/termtrace/blob/main/install.sh",
			errVersionMismatch,
			info.Version,
			binary,
			Version,
		), conn.Close())
	}

	return client, nil
}

func resolveBinary(binary string) (string, error) {
	if binary != "" {
		return binary, nil
	}

	if binary := os.Getenv("TERMTRACE_BIN"); binary != "" {
		return binary, nil
	}

	path, err := exec.LookPath("termtrace")
	if err != nil {
		return "", fmt.Errorf(
			"termtrace: CLI not found on PATH; install it with "+
				"https://github.com/zigai/termtrace/blob/main/install.sh or set TERMTRACE_BIN: %w",
			err,
		)
	}

	return path, nil
}

// Close stops the server. Recordings still open keep running in the background, as after
// [Recording.Detach].
func (c *Client) Close() error {
	return c.conn.Close() //nolint:wrapcheck // rpc errors are already termtrace errors.
}

func call[R any](ctx context.Context, c *Client, method string, params any) (R, error) {
	var result R

	raw, err := c.conn.Call(ctx, method, params)
	if err != nil {
		return result, failure(err)
	}

	if err := json.Unmarshal(raw, &result, decodeOptions); err != nil {
		return result, fmt.Errorf("termtrace: decode %s result: %w", method, err)
	}

	return result, nil
}

func callUnit(ctx context.Context, c *Client, method string, params any) error {
	_, err := c.conn.Call(ctx, method, params)
	return failure(err)
}
