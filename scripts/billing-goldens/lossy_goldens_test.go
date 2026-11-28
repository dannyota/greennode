package cli

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"unicode/utf8"
)

func TestBillingLossyGoldens(t *testing.T) {
	out := os.Getenv("BILLING_GOLDEN_DIR")
	if out == "" {
		t.Fatal("BILLING_GOLDEN_DIR is required")
	}
	resources, err := os.ReadFile("../../testdata/billing/ListResources.json")
	if err != nil {
		t.Fatal(err)
	}
	type golden struct {
		Command string `json:"command"`
		Input   string `json:"input"`
		Format  string `json:"format"`
		Query   string `json:"query"`
		Stdout  string `json:"stdout"`
	}
	var cases []golden
	for _, input := range []struct{ name, value string }{
		{"surrogate", `"\ud800"`},
		{"invalid-byte", "\"\xff\""},
	} {
		for _, test := range []struct {
			command, route, query, body string
			args                        []string
		}{
			{"list-resources", "resources", "Items[0].Status",
				strings.Replace(string(resources), `"status": 1`, `"status": `+input.value, 1), nil},
			{"list-transactions", "transactions", "Items[0].Description",
				strings.Replace(billingHistoryBody(false), `"description":"synthetic-description"`, `"description":`+input.value, 1),
				[]string{"--start-date", "2024-11-01", "--end-date", "2024-11-30"}},
		} {
			for _, output := range []struct{ format, query string }{
				{"json", ""}, {"text", ""}, {"json", test.query},
			} {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					"/gateway/api/v1/" + test.route: jsonHandler(200, test.body),
				})
				root, stdout, stderr := newSvcRoot(t, fixture)
				if test.route == "transactions" {
					root, stdout, stderr = newHistorySvcRoot(t, fixture)
				}
				args := append([]string{"--region", "hcm-3", "--output", output.format, "billing", test.command}, test.args...)
				if output.query != "" {
					args = append(args, "--query", output.query)
				}
				root.SetArgs(args)
				if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 {
					t.Fatalf("%s %s %s %q failed: %v", test.command, input.name, output.format, output.query, err)
				}
				if test.route == "resources" && input.name == "invalid-byte" && output.format == "json" && output.query == "" {
					if utf8.Valid(stdout.Bytes()) {
						t.Fatal("expected the Go RawMessage output to preserve the invalid byte")
					}
					continue
				}
				cases = append(cases, golden{test.command, input.name, output.format, output.query, stdout.String()})
			}
		}
	}
	data, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(out, "lossy-go-v1.8.0.json"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
