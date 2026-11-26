package cli

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
)

type iamWriteReply struct {
	Method  string
	Route   string
	Status  int
	Body    json.RawMessage
	RawBody *string
	Headers map[string]string
}

func TestIAMWritePortGoldens(t *testing.T) {
	writeIAMGoldens(t, "writes")
}

func TestIAMPolicyWritePortGoldens(t *testing.T) {
	writeIAMGoldens(t, "policy-writes")
}

func writeIAMGoldens(t *testing.T, subdir string) {
	t.Helper()
	dir := os.Getenv("IAM_GOLDEN_DIR")
	if dir == "" {
		t.Fatal("IAM_GOLDEN_DIR is required")
	}
	dir = filepath.Join(dir, subdir)
	input, err := os.ReadFile(filepath.Join(dir, "cases.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name    string
		Args    []string
		Exit    int
		Replies []iamWriteReply
	}
	if err := json.Unmarshal(input, &cases); err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		formats := []string{"json", "table", "text"}
		if c.Exit != 0 {
			formats = formats[:1]
		}
		for _, format := range formats {
			t.Run(c.Name+"/"+format, func(t *testing.T) {
				var requests atomic.Int32
				handlers := make(map[string]func(http.ResponseWriter, *http.Request))
				for _, reply := range c.Replies {
					handlers[strings.SplitN(reply.Route, "?", 2)[0]] = func(w http.ResponseWriter, r *http.Request) {
						index := int(requests.Add(1)) - 1
						if index >= len(c.Replies) {
							t.Error("unexpected extra request")
							w.WriteHeader(http.StatusBadRequest)
							return
						}
						step := c.Replies[index]
						if r.Method != step.Method || r.URL.RequestURI() != step.Route {
							t.Errorf("request %d: %s %s", index+1, r.Method, r.URL.Path)
							w.WriteHeader(http.StatusBadRequest)
							return
						}
						if step.Status == http.StatusNoContent {
							w.WriteHeader(step.Status)
							return
						}
						body := string(step.Body)
						if step.RawBody != nil {
							body = *step.RawBody
						}
						for header, value := range step.Headers {
							w.Header().Set(header, value)
						}
						jsonHandler(step.Status, body)(w, r)
					}
				}
				fixture := newSvcFixture(handlers)
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--output", format, "iam"}
				root.SetArgs(append(args, c.Args...))
				err := root.ExecuteContext(context.Background())
				if exitCode(err) != c.Exit || fixture.requestCount() != len(c.Replies) {
					t.Fatalf("exit = %d, requests = %d", exitCode(err), fixture.requestCount())
				}
				if err != nil {
					printError(stderr, err)
					if err := os.WriteFile(filepath.Join(dir, c.Name+".stderr.golden"), stderr.Bytes(), 0o644); err != nil {
						t.Fatal(err)
					}
				}
				if err := os.WriteFile(filepath.Join(dir, c.Name+"."+format+".golden"), stdout.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
