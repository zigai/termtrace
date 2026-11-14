package rpc

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"strings"
	"testing"
	"time"
)

type sent struct {
	ID     int64          `json:"id"`
	Method string         `json:"method"`
	Params jsontext.Value `json:"params"`
}

// fakeServer is the server end of a [Conn] built over two pipes.
type fakeServer struct {
	t         *testing.T
	requests  *bufio.Scanner
	responses io.WriteCloser
}

func connect(t *testing.T) (*Conn, *fakeServer) {
	t.Helper()

	requestReader, requestWriter := io.Pipe()
	responseReader, responseWriter := io.Pipe()
	conn := New(responseReader, requestWriter)
	server := &fakeServer{
		t:         t,
		requests:  bufio.NewScanner(requestReader),
		responses: responseWriter,
	}
	t.Cleanup(func() {
		_ = responseWriter.Close()
		_ = requestReader.Close()

		<-conn.done
	})

	return conn, server
}

func (s *fakeServer) next() sent {
	s.t.Helper()

	if !s.requests.Scan() {
		s.t.Fatalf("no request: %v", s.requests.Err())
	}

	var message sent
	if err := json.Unmarshal(s.requests.Bytes(), &message); err != nil {
		s.t.Fatalf("request %s: %v", s.requests.Bytes(), err)
	}

	return message
}

func (s *fakeServer) send(line string) {
	s.t.Helper()

	if _, err := io.WriteString(s.responses, line+"\n"); err != nil {
		s.t.Fatal(err)
	}
}

func (s *fakeServer) result(id int64, result string) {
	s.send(fmt.Sprintf(`{"jsonrpc":"2.0","id":%d,"result":%s}`, id, result))
}

type outcome struct {
	result jsontext.Value
	err    error
}

func start(ctx context.Context, conn *Conn, method string) chan outcome {
	done := make(chan outcome, 1)

	go func() {
		result, err := conn.Call(ctx, method, struct{}{})
		done <- outcome{result, err}
	}()

	return done
}

func receive(t *testing.T, done chan outcome) outcome {
	t.Helper()

	select {
	case result := <-done:
		return result
	case <-time.After(5 * time.Second):
		t.Fatal("call did not return")
		return outcome{}
	}
}

func TestResponsesReachTheirCallsInAnyOrder(t *testing.T) {
	conn, server := connect(t)
	first := start(t.Context(), conn, "first")
	firstRequest := server.next()
	second := start(t.Context(), conn, "second")
	secondRequest := server.next()

	server.result(secondRequest.ID, `"second result"`)
	server.result(firstRequest.ID, `"first result"`)

	for done, want := range map[chan outcome]string{first: `"first result"`, second: `"second result"`} {
		got := receive(t, done)
		if got.err != nil || string(got.result) != want {
			t.Errorf("got %s, %v; want %s", got.result, got.err, want)
		}
	}
}

func TestErrorResponsesKeepCodeAndData(t *testing.T) {
	conn, server := connect(t)
	done := start(t.Context(), conn, "fails")
	request := server.next()
	server.send(
		fmt.Sprintf(
			`{"jsonrpc":"2.0","id":%d,"error":{"code":-32000,"message":"no match","data":{"kind":"no_match"}}}`,
			request.ID,
		),
	)

	var failure *Error
	if err := receive(t, done).err; !errors.As(err, &failure) {
		t.Fatalf("error = %v", err)
	}

	if failure.Code != -32000 || failure.Message != "no match" ||
		string(failure.Data) != `{"kind":"no_match"}` {
		t.Errorf("error = %+v", failure)
	}
}

func TestServerExitFailsEveryPendingCall(t *testing.T) {
	conn, server := connect(t)
	first := start(t.Context(), conn, "first")

	server.next()

	second := start(t.Context(), conn, "second")

	server.next()

	if err := server.responses.Close(); err != nil {
		t.Fatal(err)
	}

	for _, done := range []chan outcome{first, second} {
		if err := receive(t, done).err; !errors.Is(err, ErrClosed) {
			t.Errorf("pending call = %v, want ErrClosed", err)
		}
	}

	if _, err := conn.Call(t.Context(), "later", struct{}{}); !errors.Is(err, ErrClosed) {
		t.Errorf("later call = %v, want ErrClosed", err)
	}
}

func TestMalformedResponseFailsPendingCalls(t *testing.T) {
	conn, server := connect(t)
	done := start(t.Context(), conn, "call")

	server.next()
	server.send(`not json`)

	if err := receive(
		t,
		done,
	).err; err == nil ||
		!strings.Contains(err.Error(), "malformed server response") {
		t.Errorf("call = %v", err)
	}
}

func TestCancelledCallAsksTheServerToCancelAndDropsTheLateResult(t *testing.T) {
	conn, server := connect(t)
	ctx, cancel := context.WithCancel(t.Context())
	abandoned := start(ctx, conn, "slow")
	slow := server.next()

	cancel()

	notice := server.next()
	if notice.Method != "$/cancel" || notice.ID != 0 ||
		string(notice.Params) != fmt.Sprintf(`{"id":%d}`, slow.ID) {
		t.Fatalf("cancel notice = %+v", notice)
	}

	if err := receive(t, abandoned).err; !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled call = %v", err)
	}

	next := start(t.Context(), conn, "next")
	nextRequest := server.next()
	server.result(slow.ID, `"late"`)
	server.result(nextRequest.ID, `"next result"`)

	if got := receive(t, next); got.err != nil || string(got.result) != `"next result"` {
		t.Errorf("next call = %s, %v", got.result, got.err)
	}
}
