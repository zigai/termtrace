package termtrace

import (
	"bufio"
	"encoding/json/v2"
	"fmt"
	"os"
	"strings"
	"testing"
)

const fakeServerVersion = "TERMTRACE_TEST_SERVER_VERSION"

// TestMain lets the test binary act as an SDK server that reports a chosen version.
func TestMain(m *testing.M) {
	if version, ok := os.LookupEnv(fakeServerVersion); ok {
		serveVersion(version)
		return
	}

	os.Exit(m.Run())
}

func serveVersion(version string) {
	lines := bufio.NewScanner(os.Stdin)
	for lines.Scan() {
		var request struct {
			ID     int64  `json:"id"`
			Method string `json:"method"`
		}
		if err := json.Unmarshal(
			lines.Bytes(),
			&request,
		); err != nil ||
			request.Method != "initialize" {
			continue
		}

		fmt.Printf(`{"jsonrpc":"2.0","id":%d,"result":{"version":%q}}`+"\n", request.ID, version)
	}

	if err := lines.Err(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func TestNewRejectsAServerOfAnotherVersion(t *testing.T) {
	t.Setenv(fakeServerVersion, "0.0.0-other")

	client, err := New(t.Context(), Options{Binary: os.Args[0]})
	if err == nil {
		_ = client.Close()

		t.Fatal("New accepted a mismatched server")
	}

	for _, part := range []string{"0.0.0-other", Version, "install.sh"} {
		if !strings.Contains(err.Error(), part) {
			t.Errorf("error %q does not mention %q", err, part)
		}
	}
}

func TestNewAcceptsAServerOfThisVersion(t *testing.T) {
	t.Setenv(fakeServerVersion, Version)

	client, err := New(t.Context(), Options{Binary: os.Args[0]})
	if err != nil {
		t.Fatal(err)
	}

	if err := client.Close(); err != nil {
		t.Errorf("close: %v", err)
	}
}

func TestNewReportsAMissingCLI(t *testing.T) {
	t.Setenv("TERMTRACE_BIN", "")
	t.Setenv("PATH", t.TempDir())

	_, err := New(t.Context(), Options{})
	if err == nil || !strings.Contains(err.Error(), "install.sh") ||
		!strings.Contains(err.Error(), "TERMTRACE_BIN") {
		t.Errorf("error = %v", err)
	}
}
