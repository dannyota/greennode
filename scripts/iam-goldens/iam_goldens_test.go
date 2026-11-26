package cli

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestIAMPortGoldens(t *testing.T) {
	dir := os.Getenv("IAM_GOLDEN_DIR")
	if dir == "" {
		t.Fatal("IAM_GOLDEN_DIR is required")
	}
	read := func(name string) []byte {
		data, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	var cases []struct {
		Name         string
		Args, Routes []string
		Status, Exit int
	}
	if err := json.Unmarshal(read("cases.json"), &cases); err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		formats := []string{"json", "table", "text"}
		if c.Exit != 0 {
			formats = formats[:1]
		}
		for _, format := range formats {
			t.Run(c.Name+"/"+format, func(t *testing.T) {
				handlers := make(map[string]func(http.ResponseWriter, *http.Request))
				for i, route := range c.Routes {
					path, _, _ := strings.Cut(route, "?")
					handlers[path] = jsonHandler(c.Status, string(read(fmt.Sprintf("%s.%d.response.json", c.Name, i+1))))
				}
				fixture := newSvcFixture(handlers)
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--read-only", "--output", format, "iam"}
				root.SetArgs(append(args, c.Args...))
				err := root.ExecuteContext(context.Background())
				if exitCode(err) != c.Exit {
					t.Fatalf("exit = %d, want %d", exitCode(err), c.Exit)
				}
				name, data := c.Name+"."+format+".golden", stdout.Bytes()
				if err != nil {
					printError(stderr, err)
					name, data = c.Name+".stderr.golden", stderr.Bytes()
				}
				if err := os.WriteFile(filepath.Join(dir, name), data, 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
