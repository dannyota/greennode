package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
)

func TestTaggingWriteGoldens(t *testing.T) {
	for _, op := range []string{"tag-resource", "untag-resource"} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(op+"/"+format, func(t *testing.T) {
				var reads atomic.Int32
				handler := func(w http.ResponseWriter, r *http.Request) {
					if r.Method == http.MethodPut {
						w.WriteHeader(http.StatusOK)
						return
					}
					body := `[{"key":"env","value":"old","systemTag":false}]`
					if reads.Add(1) > 1 {
						if op == "tag-resource" {
							body = `[{"key":"env","value":"new","systemTag":false}]`
						} else {
							body = `[]`
						}
					}
					jsonHandler(http.StatusOK, body)(w, r)
				}
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){"/v2/project-1/tag/resource/res-1": handler})
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--project-id", "project-1", "--output", format, "tagging", op, "--resource-id", "res-1", "--resource-type", "SERVER", "--key", "env"}
				if op == "tag-resource" {
					args = append(args, "--value", "new")
				}
				root.SetArgs(args)
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if fixture.requestCount() != 3 || stderr.Len() != 0 {
					t.Fatal("expected three requests and empty stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("TAGGING_GOLDEN_DIR"), op+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestTaggingReadGoldens(t *testing.T) {
	body, err := os.ReadFile("../../testdata/tagging/list_resource_tags.json")
	if err != nil {
		t.Fatal(err)
	}
	for _, format := range []string{"json", "table", "text"} {
		t.Run(format, func(t *testing.T) {
			fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){"/v2/project-1/tag/resource/res-1": jsonHandler(http.StatusOK, string(body))})
			root, stdout, stderr := newSvcRoot(t, fixture)
			root.SetArgs([]string{"--region", "hcm-3", "--project-id", "project-1", "--output", format, "tagging", "list-resource-tags", "--resource-id", "res-1"})
			if err := root.ExecuteContext(context.Background()); err != nil {
				t.Fatal(err)
			}
			if fixture.requestCount() != 1 || stderr.Len() != 0 {
				t.Fatal("expected one request and empty stderr")
			}
			if err := os.WriteFile(filepath.Join(os.Getenv("TAGGING_GOLDEN_DIR"), "list-resource-tags."+format+".golden"), stdout.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestTaggingSystemTagGoldens(t *testing.T) {
	cases := []struct {
		name, body string
		args       []string
	}{
		{"system-tag-prefix", "", []string{"tag-resource", "--resource-id", "res-1", "--resource-type", "SERVER", "--key", "vng.zone", "--value", "new"}},
		{"system-tag-untag-prefix", "", []string{"untag-resource", "--resource-id", "res-1", "--resource-type", "SERVER", "--key", "vng.a\"b\u0085\u00e9"}},
		{"system-tag-resource", `[{"key":"zone","value":"a","systemTag":true}]`, []string{"tag-resource", "--resource-id", "res-1", "--resource-type", "SERVER", "--key", "zone", "--value", "new"}},
		{"system-tag-untag-resource", `[{"key":"zone","value":"a","systemTag":true}]`, []string{"untag-resource", "--resource-id", "res-1", "--resource-type", "SERVER", "--key", "zone"}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			handler := jsonHandler(http.StatusOK, c.body)
			fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){"/v2/project-1/tag/resource/res-1": handler})
			root, stdout, stderr := newSvcRoot(t, fixture)
			root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "project-1", "tagging"}, c.args...))
			err := root.ExecuteContext(context.Background())
			if err == nil || stdout.Len() != 0 {
				t.Fatal("expected a failure with empty stdout")
			}
			printError(stderr, err)
			if err := os.WriteFile(filepath.Join(os.Getenv("TAGGING_GOLDEN_DIR"), c.name+".stderr.golden"), stderr.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		})
	}
}
