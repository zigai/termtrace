package consumer

import (
	"context"
	"encoding/json/v2"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"testing"
	"time"

	termtrace "github.com/zigai/termtrace/go"
)

func repo(t *testing.T) string {
	t.Helper()

	root, err := filepath.Abs(filepath.Join("..", "..", ".."))
	if err != nil {
		t.Fatal(err)
	}

	return root
}

func binary(t *testing.T) string {
	t.Helper()

	if path := os.Getenv("TERMTRACE_BIN"); path != "" {
		absolute, err := filepath.Abs(path)
		if err != nil {
			t.Fatal(err)
		}

		return absolute
	}

	return filepath.Join(repo(t), "target", "debug", "termtrace")
}

func fixture[T any](t *testing.T, relative string) T {
	t.Helper()

	data, err := os.ReadFile(filepath.Join(repo(t), "fixtures", relative))
	if err != nil {
		t.Fatal(err)
	}

	var value T
	if err := termtrace.DecodeJSON(data, &value); err != nil {
		t.Fatalf("%s: %v", relative, err)
	}

	return value
}

// newClient starts a client whose sessions live in a fresh directory, also returned.
func newClient(t *testing.T) (*termtrace.Client, string) {
	t.Helper()
	dir := t.TempDir()

	client, err := termtrace.New(t.Context(), termtrace.Options{
		Binary:       binary(t),
		EvidenceRoot: filepath.Join(dir, "sessions"),
	})
	if err != nil {
		t.Fatal(err)
	}

	t.Cleanup(func() {
		if err := client.Close(); err != nil {
			t.Errorf("close client: %v", err)
		}
	})

	return client, dir
}

// launch starts command and stops it at cleanup when it is still active.
func launch(
	t *testing.T,
	client *termtrace.Client,
	command []string,
	options termtrace.LaunchOptions,
) *termtrace.Recording {
	t.Helper()

	recording, err := client.Launch(t.Context(), command, options)
	if err != nil {
		t.Fatal(err)
	}

	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()

		active, err := recording.IsActive(ctx)
		if err == nil && active {
			if err := recording.Stop(ctx); err != nil {
				t.Errorf("stop: %v", err)

				_ = recording.ForceClose(ctx)
			}
		}
	})

	return recording
}

func within(t *testing.T, duration time.Duration) context.Context {
	t.Helper()
	ctx, cancel := context.WithTimeout(t.Context(), duration)
	t.Cleanup(cancel)

	return ctx
}

func fatal(t *testing.T, err error) {
	t.Helper()

	if err != nil {
		t.Fatal(err)
	}
}

func termtraceError(t *testing.T, err error) *termtrace.Error {
	t.Helper()

	var failure *termtrace.Error
	if !errors.As(err, &failure) {
		t.Fatalf("want a *termtrace.Error, got %T: %v", err, err)
	}

	return failure
}

// facts are the shared error facts every SDK reports.
type facts struct {
	Kind   string         `json:"kind"`
	Code   string         `json:"code"`
	Effect map[string]any `json:"effect"`
	Retry  string         `json:"retry"`
}

func errorFacts(t *testing.T, err error) facts {
	t.Helper()

	failure := termtraceError(t, err)
	if failure.Facts == nil {
		t.Fatalf("error has no facts: %v", err)
	}

	effect := map[string]any{}

	if failure.Facts.Effect != nil {
		data, err := json.Marshal(failure.Facts.Effect)
		fatal(t, err)
		fatal(t, json.Unmarshal(data, &effect))
	}

	retry := ""
	if failure.Facts.Retry != nil {
		retry = string(*failure.Facts.Retry)
	}

	return facts{Kind: failure.Facts.Kind, Code: failure.Facts.Code, Effect: effect, Retry: retry}
}

func sameFacts(left facts, right facts) bool {
	return left.Kind == right.Kind && left.Code == right.Code && left.Retry == right.Retry &&
		left.Effect["kind"] == right.Effect["kind"]
}

// reported returns the arrival times the input reporter printed for event.
func reported(text string, event string) []int {
	pattern := regexp.MustCompile(`(?m)^` + regexp.QuoteMeta(event) + ` T=(\d+)`)

	matches := pattern.FindAllStringSubmatch(text, -1)
	times := make([]int, 0, len(matches))

	for _, match := range matches {
		value, _ := strconv.Atoi(match[1])
		times = append(times, value)
	}

	return times
}

type interaction struct {
	EchoCommand        []string         `json:"echo_command"`
	RawCommand         []string         `json:"raw_command"`
	MouseCommand       []string         `json:"mouse_command"`
	EnvironmentCommand []string         `json:"environment_command"`
	ReadyText          string           `json:"ready_text"`
	RawBytesHex        string           `json:"raw_bytes_hex"`
	RawBytesEcho       string           `json:"raw_bytes_echo"`
	SetBytesHex        string           `json:"set_bytes_hex"`
	SetBytesEcho       string           `json:"set_bytes_echo"`
	CopyValue          string           `json:"copy_value"`
	SecretValue        string           `json:"secret_value"`
	ActivationText     string           `json:"activation_text"`
	SendStepNamespace  string           `json:"send_step_namespace"`
	SendStepName       string           `json:"send_step_name"`
	Errors             map[string]facts `json:"errors"`
}

func interactionFixture(t *testing.T) interaction {
	t.Helper()
	return fixture[interaction](t, "conformance/actions/interaction-control.json")
}

// withEcho launches the interaction echo command and waits until it is ready.
func withEcho(t *testing.T, client *termtrace.Client, fixture interaction) *termtrace.Recording {
	t.Helper()
	recording := launch(t, client, fixture.EchoCommand, termtrace.LaunchOptions{})
	_, err := recording.Screen().WaitForText(t.Context(), fixture.ReadyText, 5*time.Second)
	fatal(t, err)

	return recording
}

func program(t *testing.T, name string) string {
	t.Helper()
	return filepath.Join(repo(t), "fixtures", "programs", "terminal", name)
}

func sameJSON(t *testing.T, left any, right any) bool {
	t.Helper()

	first, err := json.Marshal(left)
	fatal(t, err)
	second, err := json.Marshal(right)
	fatal(t, err)

	return string(first) == string(second)
}
