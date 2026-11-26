package cli

import (
    "context"
    "net/http"
    "os"
    "path/filepath"
    "testing"
)

func TestBillingTemplateGoldens(t *testing.T) {
    const budget = `{"uuid":"b-1","id":1,"name":"monthly","periodType":"MONTHLY","type":"ACTUAL","limitAmount":100,"currency":"VND","status":"ACTIVE","periodKey":"2026-01","periodStart":"2026-01-01","periodEnd":"2026-01-31","startDate":"2026-01-01","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","actualCost":null,"forecastedCost":null,"actualPercentage":null,"forecastedPercentage":null,"thresholdPercentage":null,"alarm":false,"thresholdCount":0,"alarmThresholdCount":0}`
    for _, command := range []string{"list-budgets", "get-budget"} {
        for _, format := range []string{"json", "table", "text"} {
            t.Run(command+"/"+format, func(t *testing.T) {
                body := `{"code":200,"message":"ok","data":` + budget + `}`
                path := "/gateway/api/v1/budgets/b-1"
                args := []string{"--region", "hcm-3", "--output", format, "billing", command, "--budget-uuid", "b-1"}
                if command == "list-budgets" {
                    body = `{"code":200,"message":"ok","data":[` + budget + `]}`
                    path = "/gateway/api/v1/budgets"
                    args = []string{"--region", "hcm-3", "--output", format, "billing", command}
                }
                fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){path: jsonHandler(http.StatusOK, body)})
                root, stdout, _ := newSvcRoot(t, fixture)
                root.SetArgs(args)
                if err := root.ExecuteContext(context.Background()); err != nil { t.Fatal(err) }
                if fixture.requestCount() != 1 { t.Fatal("unexpected request count") }
                if err := os.WriteFile(filepath.Join(os.Getenv("BILLING_GOLDEN_DIR"), command+"."+format+".golden"), stdout.Bytes(), 0600); err != nil { t.Fatal(err) }
            })
        }
    }
}

func TestBillingBudgetReadGoldens(t *testing.T) {
	for _, tc := range []struct {
		command, fixture, path string
		flags                  []string
	}{
		{"get-current-period-cost", "GetCurrentPeriodCost", "cost/overview", nil},
		{"list-budget-thresholds", "ListBudgetThresholds", "budget-1/thresholds", []string{"--budget-uuid", "budget-1"}},
		{"list-budget-alerts", "ListBudgetAlerts", "budget-1/alerts", []string{"--budget-uuid", "budget-1", "--period-key", "2026-09"}},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "billing", tc.fixture+".json"))
		if err != nil {
			t.Fatal(err)
		}
		path := "/gateway/api/v1/budgets/" + tc.path
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){path: jsonHandler(http.StatusOK, string(body))})
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--output", format, "billing", tc.command}
				root.SetArgs(append(args, tc.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if fixture.requestCount() != 1 || stderr.Len() != 0 {
					t.Fatal("expected one request and empty stderr")
				}
				name := tc.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("BILLING_GOLDEN_DIR"), name), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
