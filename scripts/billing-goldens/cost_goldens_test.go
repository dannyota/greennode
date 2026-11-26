package cli

import (
	"bytes"
	"context"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestBillingCostGoldens(t *testing.T) {
	dates := []string{"--start-date", "2026-11-01", "--end-date", "2026-11-09"}
	for _, operation := range []struct {
		command, path, fixture string
		flags                  []string
	}{
		{"get-cost-overview", "/gateway/api/v2/cost-explorer/overview", "GetCostOverviewCaptured", dates},
		{"list-cost-resources", "/gateway/api/v2/cost-explorer/resources", "ListCostResources", dates},
		{"get-balances", "/navbar/balances/v1", "GetBalances", nil},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "billing", operation.fixture+".json"))
		if err != nil {
			t.Fatal(err)
		}
		if operation.command == "get-balances" {
			body = bytes.Replace(body, []byte(`"cashAvailable": 950000,`), []byte(`"cashAvailable": 10000000.123456789,`), 1)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(operation.command+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					operation.path: jsonHandler(http.StatusOK, string(body)),
				})
				root, stdout, _ := newSvcRoot(t, fixture)
				root.SetArgs(append([]string{"--region", "hcm-3", "--output", format, "billing", operation.command}, operation.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if fixture.requestCount() != 1 {
					t.Fatal("unexpected request count")
				}
				name := operation.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("BILLING_GOLDEN_DIR"), name), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
