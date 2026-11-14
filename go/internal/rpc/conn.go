// Package rpc is the JSON-RPC 2.0 transport to `termtrace sdk-server`: one JSON object per
// line on the server's stdin and stdout.
package rpc

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"sync"
)

// ErrClosed reports a call on a connection that is closed or whose server exited.
var ErrClosed = errors.New("termtrace: sdk server connection is closed")

// Error is an error response from the server.
type Error struct {
	// Code is the JSON-RPC error code.
	Code int64 `json:"code"`
	// Message describes the failure.
	Message string `json:"message"`
	// Data holds typed failure facts, when the server sent them.
	Data jsontext.Value `json:"data,omitzero"`
}

type request struct {
	JSONRPC string `json:"jsonrpc"`
	ID      int64  `json:"id"`
	Method  string `json:"method"`
	Params  any    `json:"params"`
}

type notification struct {
	JSONRPC string `json:"jsonrpc"`
	Method  string `json:"method"`
	Params  any    `json:"params"`
}

type response struct {
	ID     *int64         `json:"id"`
	Result jsontext.Value `json:"result"`
	Error  *Error         `json:"error"`
}

type cancelParams struct {
	ID int64 `json:"id"`
}

// Conn multiplexes concurrent calls over one server.
type Conn struct {
	input   io.WriteCloser
	process *exec.Cmd

	writeMu sync.Mutex //exhaustruct:optional

	mu      sync.Mutex //exhaustruct:optional
	nextID  int64      //exhaustruct:optional
	pending map[int64]chan response
	failure error //exhaustruct:optional

	done chan struct{}
}

// Start runs `binary args...` and connects to its stdin and stdout. Its stderr goes to stderr.
// The process keeps running after ctx ends, until [Conn.Close].
func Start(ctx context.Context, binary string, args []string, stderr io.Writer) (*Conn, error) {
	process := exec.CommandContext(context.WithoutCancel(ctx), binary, args...)
	process.Stderr = stderr

	input, err := process.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("termtrace: start %s: %w", binary, err)
	}

	output, err := process.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("termtrace: start %s: %w", binary, err)
	}

	if err := process.Start(); err != nil {
		return nil, fmt.Errorf("termtrace: start %s: %w", binary, err)
	}

	return newConn(output, input, process), nil
}

// New connects over an existing reader and writer, such as the ends of a pipe in a test.
func New(output io.Reader, input io.WriteCloser) *Conn {
	return newConn(output, input, nil)
}

func newConn(output io.Reader, input io.WriteCloser, process *exec.Cmd) *Conn {
	conn := &Conn{
		input:   input,
		process: process,
		pending: map[int64]chan response{},
		done:    make(chan struct{}),
	}
	go conn.read(output)

	return conn
}

func (e *Error) Error() string {
	return e.Message
}

// Call sends one request and waits for its result. When ctx ends first, Call returns an error
// that wraps ctx.Err() and asks the server to cancel the request; the server finishes any call
// it cannot cancel and drops the result.
func (c *Conn) Call(ctx context.Context, method string, params any) (jsontext.Value, error) {
	c.mu.Lock()
	if c.pending == nil {
		failure := c.failure
		c.mu.Unlock()

		return nil, failure
	}

	c.nextID++
	id := c.nextID
	waiter := make(chan response, 1)
	c.pending[id] = waiter
	c.mu.Unlock()

	if err := c.write(request{JSONRPC: "2.0", ID: id, Method: method, Params: params}); err != nil {
		c.forget(id)
		return nil, fmt.Errorf("termtrace: send %s: %w", method, err)
	}

	select {
	case message, ok := <-waiter:
		if !ok {
			return nil, c.closedError()
		}

		if message.Error != nil {
			return nil, message.Error
		}

		return message.Result, nil
	case <-ctx.Done():
		c.forget(id)
		// The cancel is advisory: a lost notification only means the server finishes the call.
		_ = c.write(notification{JSONRPC: "2.0", Method: "$/cancel", Params: cancelParams{ID: id}})

		return nil, fmt.Errorf("termtrace: call %s: %w", method, ctx.Err())
	}
}

// Close ends the input, which makes the server release its handles and exit, then waits for
// the server to exit.
func (c *Conn) Close() error {
	closeErr := c.input.Close()
	<-c.done

	if c.process != nil {
		if err := c.process.Wait(); err != nil {
			return fmt.Errorf("termtrace: sdk server exited: %w", err)
		}
	}

	if closeErr != nil {
		return fmt.Errorf("termtrace: close sdk server input: %w", closeErr)
	}

	return nil
}

func (c *Conn) read(output io.Reader) {
	reader := bufio.NewReader(output)
	failure := ErrClosed

	for {
		line, err := reader.ReadBytes('\n')
		if len(line) > 0 {
			var message response
			if decodeErr := json.Unmarshal(line, &message); decodeErr != nil {
				failure = fmt.Errorf("termtrace: malformed server response: %w", decodeErr)
				break
			}

			if message.ID != nil {
				c.deliver(*message.ID, message)
			}
		}

		if err != nil {
			if !errors.Is(err, io.EOF) {
				failure = fmt.Errorf("termtrace: read server response: %w", err)
			}

			break
		}
	}

	c.mu.Lock()
	c.failure = failure
	pending := c.pending
	c.pending = nil
	c.mu.Unlock()

	for _, waiter := range pending {
		close(waiter)
	}

	close(c.done)
}

func (c *Conn) deliver(id int64, message response) {
	c.mu.Lock()
	waiter, ok := c.pending[id]
	delete(c.pending, id)
	c.mu.Unlock()

	if ok {
		waiter <- message
	}
}

func (c *Conn) write(message any) error {
	line, err := json.Marshal(message)
	if err != nil {
		return fmt.Errorf("encode: %w", err)
	}

	line = append(line, '\n')

	c.writeMu.Lock()
	defer c.writeMu.Unlock()

	if _, err := c.input.Write(line); err != nil {
		return fmt.Errorf("write to server: %w", err)
	}

	return nil
}

func (c *Conn) forget(id int64) {
	c.mu.Lock()
	delete(c.pending, id)
	c.mu.Unlock()
}

func (c *Conn) closedError() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	return c.failure
}
