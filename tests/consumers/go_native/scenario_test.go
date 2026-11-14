package consumer

import (
	"bytes"
	"encoding/json/v2"
	"encoding/xml"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"

	termtrace "github.com/zigai/termtrace/go"
)

//exhaustruct:optional
type scenarioDocument struct {
	env       map[string]string
	matrix    map[string][]string
	timeoutMs int
}

func writeScenario(
	t *testing.T,
	file string,
	name string,
	script string,
	steps []map[string]any,
	options scenarioDocument,
) string {
	t.Helper()

	env := options.env
	if env == nil {
		env = map[string]string{}
	}

	timeout := options.timeoutMs
	if timeout == 0 {
		timeout = 5000
	}

	document := map[string]any{
		"version": 2,
		"name":    name,
		"launch": map[string]any{
			"command":  []string{"/bin/sh", "-c", script},
			"geometry": map[string]int{"cols": 40, "rows": 4},
			"environment": map[string]any{
				"base": "clear",
				"copy": []string{"PATH"},
				"set":  env,
			},
			"timeoutMs": timeout,
		},
		"steps": steps,
	}
	if options.matrix != nil {
		document["matrix"] = options.matrix
	}

	data, err := json.Marshal(document)
	fatal(t, err)
	fatal(t, os.WriteFile(file, data, 0o600))

	return file
}

func waitStep(text string) map[string]any {
	return map[string]any{"waitScreen": map[string]any{"contains": text}}
}

func stopStep() map[string]any {
	return map[string]any{"stop": map[string]any{}}
}

func readyScenario(t *testing.T, file string, name string, delay string) string {
	t.Helper()

	return writeScenario(
		t,
		file,
		name,
		"sleep "+delay+"; printf READY; read line",
		[]map[string]any{waitStep("READY"), stopStep()},
		scenarioDocument{},
	)
}

func matrixScenario(t *testing.T, directory string) (string, string) {
	t.Helper()

	created := filepath.Join(directory, "created")
	fatal(t, os.Mkdir(created, 0o700))

	document := writeScenario(
		t,
		filepath.Join(directory, "matrix.json"),
		"matrix",
		`printf x > "$CREATED/{{size}}-{{theme}}"; printf READY; read line`,
		[]map[string]any{waitStep("READY"), stopStep()},
		scenarioDocument{
			env:    map[string]string{"CREATED": created},
			matrix: map[string][]string{"size": {"small", "large"}, "theme": {"dark", "light"}},
		},
	)

	return document, created
}

func attemptTotal(report termtrace.ScenarioSuiteReport) uint64 {
	var total uint64

	for _, scenarioCase := range report.Cases {
		for _, attempt := range scenarioCase.Attempts {
			total += attempt.DurationMs
		}
	}

	return total
}

func caseScenarios(report termtrace.ScenarioSuiteReport) []string {
	scenarios := make([]string, 0, len(report.Cases))
	for _, scenarioCase := range report.Cases {
		scenarios = append(scenarios, string(scenarioCase.Scenario))
	}

	slices.Sort(scenarios)

	return scenarios
}

func wantRejected(t *testing.T, err error, text string) {
	t.Helper()

	if err == nil {
		t.Fatalf("want an error mentioning %q", text)
	}

	if !strings.Contains(termtraceError(t, err).Message, text) {
		t.Fatalf("want an error mentioning %q, got %v", text, err)
	}
}

func TestScenarioParallelJobsOverlapCases(t *testing.T) {
	client, directory := newClient(t)

	scenarios := make([]string, 0, 3)
	for _, index := range []string{"0", "1", "2"} {
		scenarios = append(scenarios, readyScenario(
			t,
			filepath.Join(directory, "slow-"+index+".json"),
			"slow "+index,
			"1",
		))
	}

	parallel, err := client.RunScenarios(t.Context(), scenarios, termtrace.ScenarioSuiteOptions{
		FailureRoot: filepath.Join(directory, "parallel", "failures"),
		Jobs:        3,
	})
	fatal(t, err)

	if parallel.Jobs != 3 {
		t.Fatalf("jobs = %d, want 3", parallel.Jobs)
	}

	if got := caseScenarios(parallel); !slices.Equal(got, scenarios) {
		t.Fatalf("cases = %v, want %v", got, scenarios)
	}

	for _, scenarioCase := range parallel.Cases {
		if scenarioCase.Status != termtrace.ScenarioCaseStatusPassed {
			t.Fatalf("case %s status = %s", scenarioCase.Scenario, scenarioCase.Status)
		}
	}

	if total := attemptTotal(parallel); parallel.DurationMs*3 >= total*2 {
		t.Fatalf("parallel suite took %d ms for %d ms of attempts", parallel.DurationMs, total)
	}

	serial, err := client.RunScenarios(t.Context(), scenarios, termtrace.ScenarioSuiteOptions{
		FailureRoot: filepath.Join(directory, "serial", "failures"),
		Jobs:        1,
	})
	fatal(t, err)

	if len(serial.Cases) != 3 {
		t.Fatalf("serial cases = %d, want 3", len(serial.Cases))
	}

	if total := attemptTotal(serial); serial.DurationMs < total {
		t.Fatalf("serial suite took %d ms for %d ms of attempts", serial.DurationMs, total)
	}
}

func TestScenarioMatrixExpandsCases(t *testing.T) {
	client, directory := newClient(t)
	document, created := matrixScenario(t, directory)

	report, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{
			FailureRoot: filepath.Join(directory, "out", "failures"),
			Jobs:        4,
		},
	)
	fatal(t, err)

	if report.Status != termtrace.ScenarioSuiteStatusPassed {
		t.Fatalf("status = %s", report.Status)
	}

	entries, err := os.ReadDir(created)
	fatal(t, err)

	files := make([]string, 0, len(entries))
	for _, entry := range entries {
		files = append(files, entry.Name())
	}

	want := []string{"large-dark", "large-light", "small-dark", "small-light"}
	if !slices.Equal(files, want) {
		t.Fatalf("created = %v, want %v", files, want)
	}

	sessions := map[string]bool{}

	for _, size := range []string{"small", "large"} {
		for _, theme := range []string{"dark", "light"} {
			parts := []string{"size=" + size, "theme=" + theme}
			names, identities := 0, 0

			for _, scenarioCase := range report.Cases {
				if containsAll(scenarioCase.Attempts[0].Report.Name, parts) {
					names++
				}

				if containsAll(string(scenarioCase.Scenario), parts) {
					identities++
				}
			}

			if names != 1 || identities != 1 {
				t.Fatalf("%v: %d names and %d identities, want 1 each", parts, names, identities)
			}
		}
	}

	for _, scenarioCase := range report.Cases {
		if session := scenarioCase.Attempts[0].Report.CanonicalSession; session != nil {
			sessions[string(*session)] = true
		}
	}

	if len(sessions) != 4 {
		t.Fatalf("distinct sessions = %d, want 4", len(sessions))
	}
}

func containsAll(text string, parts []string) bool {
	for _, part := range parts {
		if !strings.Contains(text, part) {
			return false
		}
	}

	return true
}

func TestScenarioShardsPartitionCases(t *testing.T) {
	client, directory := newClient(t)
	document, _ := matrixScenario(t, directory)

	everything, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{
			FailureRoot: filepath.Join(directory, "all", "failures"),
			Jobs:        4,
		},
	)
	fatal(t, err)

	selected := make([][]string, 0, 2)

	for index := range 2 {
		shard, err := client.RunScenarios(
			t.Context(),
			[]string{document},
			termtrace.ScenarioSuiteOptions{
				FailureRoot: filepath.Join(directory, "shard-"+strconv.Itoa(index), "failures"),
				Jobs:        2,
				Shard:       &termtrace.ScenarioShard{Index: index, Count: 2},
			},
		)
		fatal(t, err)

		if shard.ShardIndex == nil || *shard.ShardIndex != uint64(index) ||
			shard.ShardCount == nil || *shard.ShardCount != 2 {
			t.Fatalf("shard %d reported index %v of %v", index, shard.ShardIndex, shard.ShardCount)
		}

		cases := caseScenarios(shard)
		if len(cases) == 0 {
			t.Fatalf("shard %d selected no cases", index)
		}

		selected = append(selected, cases)
	}

	for _, identity := range selected[0] {
		if slices.Contains(selected[1], identity) {
			t.Fatalf("both shards selected %s", identity)
		}
	}

	union := slices.Concat(selected[0], selected[1])
	slices.Sort(union)

	if want := caseScenarios(everything); !slices.Equal(union, want) {
		t.Fatalf("shards selected %v, want %v", union, want)
	}

	_, err = client.RunScenarios(t.Context(), []string{document}, termtrace.ScenarioSuiteOptions{
		FailureRoot: filepath.Join(directory, "bad", "failures"),
		Shard:       &termtrace.ScenarioShard{Index: 2, Count: 2},
	})
	wantRejected(t, err, "shard")
}

func TestScenarioSnapshotUpdateWritesBaseline(t *testing.T) {
	client, directory := newClient(t)
	text := filepath.Join(directory, "screen.txt")
	fatal(t, os.WriteFile(text, []byte("alpha beta\n"), 0o600))

	document := writeScenario(
		t,
		filepath.Join(directory, "snapshot.json"),
		"snapshot",
		`IFS= read -r line < "$TEXT"; printf "%s" "$line"; read line`,
		[]map[string]any{
			waitStep("alpha"),
			{"matchScreenSnapshot": map[string]any{"name": "greeting", "stableForMs": 10}},
			stopStep(),
		},
		scenarioDocument{env: map[string]string{"TEXT": text}},
	)
	artifacts := filepath.Join(directory, "artifacts")
	failureRoot := filepath.Join(artifacts, "failures")

	updated, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{
			FailureRoot:     failureRoot,
			UpdateSnapshots: true,
		},
	)
	fatal(t, err)

	if updated.Status != termtrace.ScenarioSuiteStatusPassed {
		t.Fatalf("update status = %s", updated.Status)
	}

	entries, err := os.ReadDir(filepath.Join(artifacts, "snapshots"))
	fatal(t, err)

	if len(entries) != 1 {
		t.Fatalf("baselines = %d, want 1", len(entries))
	}

	baselinePath := filepath.Join(artifacts, "snapshots", entries[0].Name())
	baseline, err := os.ReadFile(baselinePath)
	fatal(t, err)

	if !bytes.Contains(baseline, []byte("alpha beta")) {
		t.Fatalf("baseline lacks the screen text: %q", baseline)
	}

	same, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{FailureRoot: failureRoot},
	)
	fatal(t, err)

	if same.Status != termtrace.ScenarioSuiteStatusPassed {
		t.Fatalf("unchanged screen status = %s", same.Status)
	}

	fatal(t, os.WriteFile(text, []byte("alpha gamma\n"), 0o600))

	changed, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{FailureRoot: failureRoot},
	)
	fatal(t, err)

	attempt := changed.Cases[0].Attempts[0].Report
	if changed.Status != termtrace.ScenarioSuiteStatusFailed {
		t.Fatalf("changed screen status = %s", changed.Status)
	}

	if attempt.Failure == nil || attempt.Failure.StepKind != "match_screen_snapshot" {
		t.Fatalf("failure = %+v, want a match_screen_snapshot failure", attempt.Failure)
	}

	requireDirectory(t, attempt.FailureArtifacts)

	after, err := os.ReadFile(baselinePath)
	fatal(t, err)

	if !bytes.Equal(after, baseline) {
		t.Fatal("a comparing run changed the baseline")
	}
}

func requireDirectory(t *testing.T, path *termtrace.Path) {
	t.Helper()

	if path == nil {
		t.Fatal("no failure artifacts")
	}

	info, err := os.Stat(string(*path))
	fatal(t, err)

	if !info.IsDir() {
		t.Fatalf("%s is not a directory", *path)
	}
}

func TestScenarioRerunsReportFlakyCases(t *testing.T) {
	client, directory := newClient(t)
	counter := filepath.Join(directory, "attempts")
	document := writeScenario(
		t,
		filepath.Join(directory, "flaky.json"),
		"passes on the second attempt",
		`n=0; if test -f "$COUNTER"; then IFS= read -r n < "$COUNTER"; fi; n=$((n + 1)); `+
			`printf "%s\n" "$n" > "$COUNTER"; if test "$n" -eq 1; then printf BROKEN; else printf DONE; fi; read line`,
		[]map[string]any{waitStep("DONE"), stopStep()},
		scenarioDocument{env: map[string]string{"COUNTER": counter}, timeoutMs: 1000},
	)

	permissive, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{
			FailureRoot: filepath.Join(directory, "a", "failures"),
			Reruns:      2,
		},
	)
	fatal(t, err)

	recovered := permissive.Cases[0]
	if permissive.Status != termtrace.ScenarioSuiteStatusFlaky ||
		recovered.Status != termtrace.ScenarioCaseStatusFlaky {
		t.Fatalf("suite %s, case %s, want flaky", permissive.Status, recovered.Status)
	}

	statuses := make([]termtrace.ScenarioStatus, 0, len(recovered.Attempts))
	for _, attempt := range recovered.Attempts {
		statuses = append(statuses, attempt.Report.Status)
	}

	if want := []termtrace.ScenarioStatus{"failed", "passed"}; !slices.Equal(statuses, want) {
		t.Fatalf("attempts = %v, want %v", statuses, want)
	}

	first, second := recovered.Attempts[0].Report, recovered.Attempts[1].Report
	if first.CanonicalSession == nil || second.CanonicalSession == nil ||
		*first.CanonicalSession == *second.CanonicalSession {
		t.Fatalf(
			"attempts share a session: %v and %v",
			first.CanonicalSession,
			second.CanonicalSession,
		)
	}

	requireDirectory(t, first.FailureArtifacts)

	count, err := os.ReadFile(counter)
	fatal(t, err)

	if string(count) != "2\n" {
		t.Fatalf("attempt counter = %q, want 2", count)
	}

	if ciFailed(t, client, permissive) {
		t.Fatal("a flaky case failed CI without fail-on-flaky")
	}

	fatal(t, os.Remove(counter))

	strict, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{
			FailureRoot: filepath.Join(directory, "b", "failures"),
			Reruns:      2,
			FailOnFlaky: true,
		},
	)
	fatal(t, err)

	if strict.Status != termtrace.ScenarioSuiteStatusFlaky || !strict.FailOnFlaky {
		t.Fatalf("strict suite %s, fail on flaky %v", strict.Status, strict.FailOnFlaky)
	}

	if !ciFailed(t, client, strict) {
		t.Fatal("a flaky case passed CI with fail-on-flaky")
	}

	fatal(t, os.Remove(counter))

	exhausted, err := client.RunScenarios(
		t.Context(),
		[]string{document},
		termtrace.ScenarioSuiteOptions{
			FailureRoot: filepath.Join(directory, "c", "failures"),
		},
	)
	fatal(t, err)

	if exhausted.Status != termtrace.ScenarioSuiteStatusFailed ||
		len(exhausted.Cases[0].Attempts) != 1 {
		t.Fatalf(
			"without reruns: %s after %d attempts",
			exhausted.Status,
			len(exhausted.Cases[0].Attempts),
		)
	}
}

func ciFailed(t *testing.T, client *termtrace.Client, report termtrace.ScenarioSuiteReport) bool {
	t.Helper()

	failed, err := client.ScenarioCIFailed(t.Context(), report)
	fatal(t, err)

	return failed
}

func mixedReport(
	t *testing.T,
	client *termtrace.Client,
	directory string,
) termtrace.ScenarioSuiteReport {
	t.Helper()

	passing := readyScenario(t, filepath.Join(directory, "a-passing.json"), "shows ready", "0")
	failing := writeScenario(
		t,
		filepath.Join(directory, "b-failing.json"),
		"never shows done",
		"printf READY; read line",
		[]map[string]any{
			{"waitScreen": map[string]any{"contains": "DONE", "timeoutMs": 500}},
			stopStep(),
		},
		scenarioDocument{},
	)

	report, err := client.RunScenarios(
		t.Context(),
		[]string{passing, failing},
		termtrace.ScenarioSuiteOptions{
			FailureRoot: filepath.Join(directory, "mixed", "failures"),
			Jobs:        2,
		},
	)
	fatal(t, err)

	return report
}

type junitSuite struct {
	XMLName  xml.Name    `xml:"testsuite"`
	Tests    string      `xml:"tests,attr"`
	Failures string      `xml:"failures,attr"`
	Errors   string      `xml:"errors,attr"`
	Cases    []junitCase `xml:"testcase"`
}

type junitCase struct {
	Name     string         `xml:"name,attr"`
	Failures []junitFailure `xml:"failure"`
}

type junitFailure struct {
	Text string `xml:",chardata"`
}

func TestScenarioJUnitMarksFailures(t *testing.T) {
	client, directory := newClient(t)
	report := mixedReport(t, client, directory)
	junit := filepath.Join(directory, "junit.xml")

	fatal(t, client.WriteScenarioJUnit(t.Context(), junit, report))

	data, err := os.ReadFile(junit)
	fatal(t, err)

	var suite junitSuite
	if err := xml.Unmarshal(data, &suite); err != nil {
		t.Fatalf("parse JUnit: %v\n%s", err, data)
	}

	if suite.Tests != "2" || suite.Failures != "1" || suite.Errors != "0" {
		t.Fatalf("tests, failures, errors = %s, %s, %s", suite.Tests, suite.Failures, suite.Errors)
	}

	cases := map[string]junitCase{}
	for _, testCase := range suite.Cases {
		cases[testCase.Name] = testCase
	}

	if len(cases) != 2 {
		t.Fatalf("cases = %v", suite.Cases)
	}

	if passing, ok := cases["shows ready"]; !ok || len(passing.Failures) != 0 {
		t.Fatalf("passing case = %+v, present %v", passing, ok)
	}

	failing, ok := cases["never shows done"]
	if !ok || len(failing.Failures) != 1 ||
		!strings.Contains(failing.Failures[0].Text, "wait_screen") {
		t.Fatalf("failing case = %+v, present %v", failing, ok)
	}

	if err := client.WriteScenarioJUnit(t.Context(), junit, report); err == nil {
		t.Fatal("overwriting an existing JUnit file succeeded")
	}

	after, err := os.ReadFile(junit)
	fatal(t, err)

	if !bytes.Equal(after, data) {
		t.Fatal("a refused write changed the JUnit file")
	}
}

func TestScenarioCIFailsOnlyOnFailedCase(t *testing.T) {
	client, directory := newClient(t)
	report := mixedReport(t, client, directory)

	if report.Status != termtrace.ScenarioSuiteStatusFailed {
		t.Fatalf("mixed status = %s", report.Status)
	}

	statuses := []termtrace.ScenarioCaseStatus{report.Cases[0].Status, report.Cases[1].Status}
	if want := []termtrace.ScenarioCaseStatus{"passed", "failed"}; !slices.Equal(statuses, want) {
		t.Fatalf("case statuses = %v, want %v", statuses, want)
	}

	if !ciFailed(t, client, report) {
		t.Fatal("a failed case passed CI")
	}

	passing, err := client.RunScenarios(
		t.Context(),
		[]string{readyScenario(t, filepath.Join(directory, "only.json"), "only", "0")},
		termtrace.ScenarioSuiteOptions{FailureRoot: filepath.Join(directory, "ok", "failures")},
	)
	fatal(t, err)

	if passing.Status != termtrace.ScenarioSuiteStatusPassed || ciFailed(t, client, passing) {
		t.Fatalf("passing suite %s failed CI", passing.Status)
	}

	relabeled := report
	relabeled.Status = termtrace.ScenarioSuiteStatusPassed

	if !ciFailed(t, client, relabeled) {
		t.Fatal("a failed case passed CI under a passed suite status")
	}
}
