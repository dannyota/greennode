package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
)

func TestIAMUserWritePortGoldens(t *testing.T) {
	dir := filepath.Join(os.Getenv("IAM_GOLDEN_DIR"), "user-writes")
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
		replies := c.Replies
		formats := []string{"json", "table", "text"}
		if c.Exit != 0 {
			formats = formats[:1]
		}
		for _, format := range formats {
			t.Run(c.Name+"/"+format, func(t *testing.T) {
				var requests atomic.Int32
				handlers := make(map[string]func(http.ResponseWriter, *http.Request))
				for _, reply := range replies {
					handlers[reply.Route] = func(w http.ResponseWriter, r *http.Request) {
						index := int(requests.Add(1)) - 1
						if index >= len(replies) {
							t.Error("unexpected extra request")
							w.WriteHeader(http.StatusBadRequest)
							return
						}
						step := replies[index]
						if r.Method != step.Method || r.URL.Path != step.Route {
							t.Errorf("request %d: %s %s", index+1, r.Method, r.URL.Path)
							w.WriteHeader(http.StatusBadRequest)
							return
						}
						if step.Status == http.StatusNoContent {
							w.WriteHeader(step.Status)
							return
						}
						jsonHandler(step.Status, string(step.Body))(w, r)
					}
				}
				fixture := newSvcFixture(handlers)
				root, stdout, stderr := newSvcRoot(t, fixture)
				dirForSecrets := t.TempDir()
				file := filepath.Join(dirForSecrets, "issued")
				password := filepath.Join(dirForSecrets, "input")
				if err := os.WriteFile(password, []byte("private-toy\n"), 0o600); err != nil {
					t.Fatal(err)
				}
				args := []string{"--region", "hcm-3", "--output", format, "iam"}
				for _, arg := range c.Args {
					if arg == "$SECRET" {
						arg = file
					} else if arg == "$PASSWORD" {
						arg = password
					}
					args = append(args, arg)
				}
				root.SetArgs(args)
				err := root.ExecuteContext(context.Background())
				if exitCode(err) != c.Exit || fixture.requestCount() != len(replies) {
					t.Fatalf("exit = %d, requests = %d", exitCode(err), fixture.requestCount())
				}
				write := func(suffix string, data []byte) {
					t.Helper()
					data = bytes.ReplaceAll(data, []byte(file), []byte("$SECRET"))
					if err := os.WriteFile(filepath.Join(dir, c.Name+suffix+".golden"), data, 0o644); err != nil {
						t.Fatal(err)
					}
				}
				if err != nil {
					printError(stderr, err)
					write(".stderr", stderr.Bytes())
				}
				write("."+format, stdout.Bytes())
			})
		}
	}
}
