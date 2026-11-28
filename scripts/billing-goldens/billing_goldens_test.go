package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"danny.vn/vngcloud"
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
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if fixture.requestCount() != 1 {
					t.Fatal("unexpected request count")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("BILLING_GOLDEN_DIR"), command+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
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

func TestBillingResourcesGoldens(t *testing.T) {
	body, err := os.ReadFile("../../testdata/billing/ListResources.json")
	if err != nil {
		t.Fatal(err)
	}
	body = bytes.Replace(body, []byte(`"cost": 123,`), []byte(`"cost": 10000000.123456789,`), 1)
	for _, format := range []string{"json", "table", "text"} {
		t.Run(format, func(t *testing.T) {
			fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
				"/gateway/api/v1/resources": jsonHandler(http.StatusOK, string(body)),
			})
			root, stdout, _ := newSvcRoot(t, fixture)
			root.SetArgs([]string{"--region", "hcm-3", "--output", format, "billing", "list-resources"})
			if err := root.ExecuteContext(context.Background()); err != nil {
				t.Fatal(err)
			}
			if fixture.requestCount() != 1 {
				t.Fatal("unexpected request count")
			}
			path := filepath.Join(os.Getenv("BILLING_GOLDEN_DIR"), "list-resources."+format+".golden")
			if err := os.WriteFile(path, stdout.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestBillingBudgetWriteGoldens(t *testing.T) {
	for _, tc := range []struct {
		command, fixture, path string
		status                 int
		flags                  []string
	}{
		{"create-budget", "CreateBudget", "", http.StatusCreated, []string{"--name", "example-budget", "--period-type", "MONTHLY", "--type", "ACTUAL", "--limit-amount", "1000000"}},
		{"update-budget", "UpdateBudget", "/budget-1", http.StatusOK, []string{"--budget-uuid", "budget-1", "--status", "PAUSED"}},
		{"delete-budget", "DeleteBudget", "/budget-1", http.StatusOK, []string{"--budget-uuid", "budget-1", "--yes"}},
		{"create-budget-threshold", "CreateBudgetThreshold", "/budget-1/thresholds", http.StatusCreated, []string{"--budget-uuid", "budget-1", "--threshold-type", "ACTUAL", "--threshold-percentage", "100"}},
		{"update-budget-threshold", "UpdateBudgetThreshold", "/budget-1/thresholds/threshold-1", http.StatusOK, []string{"--budget-uuid", "budget-1", "--threshold-uuid", "threshold-1", "--enabled=false"}},
		{"delete-budget-threshold", "DeleteBudgetThreshold", "/budget-1/thresholds/threshold-1", http.StatusOK, []string{"--budget-uuid", "budget-1", "--threshold-uuid", "threshold-1", "--yes"}},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "billing", tc.fixture+".json"))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				path := "/gateway/api/v1/budgets" + tc.path
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){path: jsonHandler(tc.status, string(body))})
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

func TestBillingValidationGoldens(t *testing.T) {
	type validationCase struct {
		Name       string   `json:"name"`
		Args       []string `json:"args"`
		HTTPStatus int      `json:"http_status"`
		Body       string   `json:"body"`
		Exit       int      `json:"exit"`
		Code       string   `json:"code,omitempty"`
		Status     int      `json:"status,omitempty"`
		Message    string   `json:"message,omitempty"`
		Bearer     string   `json:"bearer,omitempty"`
		Stdout     string   `json:"stdout,omitempty"`
	}
	transactions := []string{"list-transactions", "--start-date", "2024-11-01", "--end-date", "2024-11-30", "--size", "50"}
	payments := []string{"list-payments", "--start-date", "2024-11-01", "--end-date", "2024-11-30", "--size", "50"}
	resources := []string{"list-resources"}
	overview := []string{"get-cost-overview", "--start-date", "2026-11-01", "--end-date", "2026-11-09"}
	costResources := []string{"list-cost-resources", "--start-date", "2026-11-01", "--end-date", "2026-11-09"}
	thresholds := []string{"list-budget-thresholds", "--budget-uuid", "budget-1"}
	alerts := []string{"list-budget-alerts", "--budget-uuid", "budget-1", "--period-key", "2026-09"}
	load := func(name string) string {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "billing", name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	addField := func(body, field string) string { return strings.Replace(body, "{", "{"+field+",", 1) }
	const payment = `{"paymentID":1,"transactionID":"txn-1","payGate":"BALANCE","paymentStatus":"PAID","paymentAmount":10,"paymentType":1,"description":null,"createTimeStamp":"2024-11-02T03:04:05Z"}`
	const paymentMetadata = `{"number":1,"size":50,"totalPages":1,"totalElements":1}`
	paymentPage := func(row, page string) string {
		return `{"code":200,"data":{"content":[` + row + `],"pageable":` + page + `}}`
	}
	get := []string{"get-budget", "--budget-uuid", "b-1"}
	list := []string{"list-budgets"}
	cost := []string{"get-current-period-cost"}
	balances := []string{"get-balances"}
	paths := map[string]string{
		"get-budget":              "/gateway/api/v1/budgets/b-1",
		"list-budgets":            "/gateway/api/v1/budgets",
		"get-current-period-cost": "/gateway/api/v1/budgets/cost/overview",
		"get-balances":            "/navbar/balances/v1",
		"list-transactions":       "/gateway/api/v1/transactions",
		"list-payments":           "/gateway/api/v1/payments",
		"list-resources":          "/gateway/api/v1/resources",
		"get-cost-overview":       "/gateway/api/v2/cost-explorer/overview",
		"list-cost-resources":     "/gateway/api/v2/cost-explorer/resources",
		"list-budget-thresholds":  "/gateway/api/v1/budgets/budget-1/thresholds",
		"list-budget-alerts":      "/gateway/api/v1/budgets/budget-1/alerts",
		"create-budget":           "/gateway/api/v1/budgets",
		"update-budget":           "/gateway/api/v1/budgets/budget-1",
		"delete-budget":           "/gateway/api/v1/budgets/budget-1",
		"create-budget-threshold": "/gateway/api/v1/budgets/budget-1/thresholds",
		"update-budget-threshold": "/gateway/api/v1/budgets/budget-1/thresholds/threshold-1",
		"delete-budget-threshold": "/gateway/api/v1/budgets/budget-1/thresholds/threshold-1",
	}
	rows := []validationCase{
		{Name: "canonical budget", Args: get, HTTPStatus: 200, Body: `{"code":200,"data":{"uuid":"b-1","name":"monthly"}}`},
		{Name: "unknown single field", Args: get, HTTPStatus: 200, Body: `{"code":200,"data":{"uuid":"b-1","future":{"Name":1}}}`},
		{Name: "known data field casing", Args: get, HTTPStatus: 200, Body: `{"code":200,"data":{"uuid":"b-1","Name":"monthly"}}`},
		{Name: "typed row field casing", Args: list, HTTPStatus: 200, Body: `{"code":200,"data":[{"uuid":"b-1","Currency":"VND"}]}`},
		{Name: "folded unknown object keys", Args: get, HTTPStatus: 200, Body: `{"code":200,"data":{"uuid":"b-1","future":[{"name":1,"Name":2}]}}`},
		{Name: "unicode folded unknown keys", Args: get, HTTPStatus: 200, Body: `{"code":200,"data":{"uuid":"b-1","future":{"Σ":1,"ς":2}}}`},
		{Name: "escaped folded unknown keys", Args: get, HTTPStatus: 200, Body: `{"code":200,"data":{"uuid":"b-1","future":{"name":1,"\u004eame":2}}}`},
		{Name: "scope of folded unknown keys", Args: get, HTTPStatus: 200, Body: `{"code":200,"data":{"uuid":"b-1","future":[{"name":1},{"Name":2}]}}`},
		{Name: "transport message casing", Args: get, HTTPStatus: 400, Body: `{"Message":"Budget not found"}`},
		{Name: "first transport array message", Args: get, HTTPStatus: 400, Body: `[{"Message":"Budget not found"},{"message":"unrelated"}]`},
		{Name: "invalid later transport array item", Args: get, HTTPStatus: 400, Body: `[{"Message":"Budget not found"},{"title":7}]`},
		{Name: "valid envelope refusal", Args: get, HTTPStatus: 200, Body: `{"code":400,"message":"Budget not found","data":{"future":{"Name":1}}}`},
		{Name: "refusal with ambiguous unknown keys", Args: get, HTTPStatus: 200, Body: `{"code":400,"message":"Budget not found","data":{"future":{"Name":1,"name":2}}}`},
		{Name: "reflected refusal with ambiguous unknown keys", Args: get, HTTPStatus: 200, Body: `{"code":"Denied","message":"rejected test-token","future":{"a":1,"A":2}}`},
		{Name: "null period cost", Args: cost, HTTPStatus: 200, Body: `{"code":200,"data":{"actualCost":0,"forecastedCost":null}}`},
		{Name: "zero period cost", Args: cost, HTTPStatus: 200, Body: `{"code":200,"data":{"actualCost":0,"forecastedCost":0}}`},
		{Name: "null list row", Args: list, HTTPStatus: 200, Body: `{"code":200,"data":[null]}`},
		{Name: "nullable direct balances", Args: balances, HTTPStatus: 200, Body: `{"cash":null,"poc":null,"cashAvailable":null,"cashHolding":null,"pocHolding":null}`},
		{Name: "missing direct balance key", Args: balances, HTTPStatus: 200, Body: `{"cash":null,"poc":null,"cashAvailable":null,"cashHolding":null}`},
		{Name: "empty envelope refusal message", Args: get, HTTPStatus: 200, Body: `{"code":"Denied"}`},
		{Name: "reflected refusal without duplicate keys", Args: get, HTTPStatus: 200, Body: `{"code":"Denied","message":"rejected test-token"}`},
		{Name: "transaction history ignores folded unknown envelope fields", Args: transactions, HTTPStatus: 200, Body: addField(load("ListTransactions"), `"future":{"key":1,"Key":2}`)},
		{Name: "transaction history accepts an envelope code alias", Args: transactions, HTTPStatus: 200, Body: strings.Replace(load("ListTransactions"), `"code"`, `"Code"`, 1)},
		{Name: "payments ignore unknown row fields", Args: payments, HTTPStatus: 200, Body: strings.Replace(load("ListPayments"), `"paymentID"`, `"future":{"name":1,"Name":2},"paymentID"`, 1)},
		{Name: "payments require the exact ID field", Args: payments, HTTPStatus: 200, Body: strings.Replace(load("ListPayments"), `"paymentID"`, `"PaymentID"`, 1)},
		{Name: "history ignores an out-of-range envelope number", Args: transactions, HTTPStatus: 200, Body: addField(load("ListTransactions"), `"future":1e400`)},
		{Name: "history ignores an out-of-range page and metadata number", Args: transactions, HTTPStatus: 200, Body: strings.NewReplacer(`"content"`, `"future":1e400,"content"`, `"number"`, `"future":1e400,"number"`).Replace(load("ListTransactions"))},
		{Name: "history ignores an out-of-range row number", Args: payments, HTTPStatus: 200, Body: strings.Replace(load("ListPayments"), `"paymentID"`, `"future":1e400,"paymentID"`, 1)},
		{Name: "history applies a later known row alias", Args: payments, HTTPStatus: 200, Body: paymentPage(strings.Replace(payment, `"paymentAmount":10`, `"paymentAmount":10,"PaymentAmount":20`, 1), paymentMetadata)},
		{Name: "history applies a later exact row duplicate", Args: payments, HTTPStatus: 200, Body: paymentPage(strings.Replace(payment, `"paymentAmount":10`, `"paymentAmount":10,"paymentAmount":20`, 1), paymentMetadata)},
		{Name: "history rejects a malformed known row field before a valid alias", Args: payments, HTTPStatus: 200, Body: paymentPage(strings.Replace(payment, `"paymentAmount":10`, `"paymentAmount":"invalid","PaymentAmount":20`, 1), paymentMetadata)},
		{Name: "history rejects malformed metadata before a valid alias", Args: payments, HTTPStatus: 200, Body: paymentPage(payment, strings.Replace(paymentMetadata, `"number":1`, `"number":"invalid","Number":1`, 1))},
		{Name: "history applies a valid metadata alias", Args: payments, HTTPStatus: 200, Body: paymentPage(payment, strings.Replace(paymentMetadata, `"number":1`, `"number":2,"Number":1`, 1))},
		{Name: "short credential withholds the whole refusal", Args: get, HTTPStatus: 200, Body: `{"code":"Denied","message":"toy was rejected"}`, Bearer: "toy"},
		{Name: "refusal match budget withholds the whole field", Args: get, HTTPStatus: 200, Body: `{"code":"Denied","message":"` + strings.Repeat("test-token ", 4097) + `"}`},
		{Name: "history rejects an out-of-range known amount", Args: payments, HTTPStatus: 200, Body: `{"code":200,"data":{"content":[{"paymentID":1,"transactionID":"txn-1","payGate":"BALANCE","paymentStatus":"PAID","paymentAmount":1e400,"paymentType":1,"description":null,"createTimeStamp":"2024-11-02T03:04:05Z"}],"pageable":{"number":1,"size":50,"totalPages":1,"totalElements":1}}}`},
		{Name: "resource inventory ignores folded unknown keys", Args: resources, HTTPStatus: 200, Body: addField(load("ListResources"), `"future":{"name":1,"Name":2}`)},
		{Name: "resource inventory rejects exact unknown duplicates", Args: resources, HTTPStatus: 200, Body: addField(load("ListResources"), `"future":{"name":1,"name":2}`)},
		{Name: "resource inventory rejects known aliases", Args: resources, HTTPStatus: 200, Body: strings.Replace(load("ListResources"), `"artifactId"`, `"ArtifactId"`, 1)},
		{Name: "direct balances require an exact envelope header", Args: balances, HTTPStatus: 200, Body: `{"Message":"ok","cash":null,"poc":null,"cashAvailable":null,"cashHolding":null,"pocHolding":null}`},
		{Name: "direct balances require a typed envelope header", Args: balances, HTTPStatus: 200, Body: `{"message":9,"cash":null,"poc":null,"cashAvailable":null,"cashHolding":null,"pocHolding":null}`},
		{Name: "direct balances accept an ordinary envelope message", Args: balances, HTTPStatus: 200, Body: `{"message":"ok","cash":null,"poc":null,"cashAvailable":null,"cashHolding":null,"pocHolding":null}`},
		{Name: "grouped overview ignores flat series field casing", Args: overview, HTTPStatus: 200, Body: `{"code":200,"data":{"summary":{"currentCost":0},"series":[{"key":"group","points":[{"x":"2026-11-01","y":0}],"Date":"unused","Cost":7}]}}`},
		{Name: "grouped overview requires exact point fields", Args: overview, HTTPStatus: 200, Body: `{"code":200,"data":{"summary":{"currentCost":0},"series":[{"key":"group","points":[{"X":"2026-11-01","y":0}]}]}}`},
		{Name: "flat overview requires date and cost", Args: overview, HTTPStatus: 200, Body: `{"code":200,"data":{"summary":{"currentCost":0},"series":[{"date":"2026-11-01"}]}}`},
		{Name: "canonical flat overview", Args: overview, HTTPStatus: 200, Body: `{"code":200,"data":{"summary":{"currentCost":0},"series":[{"date":"2026-11-01","cost":0}]}}`},
		{Name: "canonical cost resources", Args: costResources, HTTPStatus: 200, Body: load("ListCostResources")},
		{Name: "cost resource case-sensitive fields", Args: costResources, HTTPStatus: 200, Body: strings.Replace(load("ListCostResources"), `"totalCost"`, `"TotalCost"`, 1)},
		{Name: "canonical thresholds", Args: thresholds, HTTPStatus: 200, Body: load("ListBudgetThresholds")},
		{Name: "threshold row case-sensitive fields", Args: thresholds, HTTPStatus: 200, Body: strings.Replace(load("ListBudgetThresholds"), `"uuid"`, `"UUID"`, 1)},
		{Name: "canonical alerts", Args: alerts, HTTPStatus: 200, Body: load("ListBudgetAlerts")},
		{Name: "canonical create budget", Args: []string{"create-budget", "--name", "sample", "--period-type", "MONTHLY", "--type", "ACTUAL", "--limit-amount", "100"}, HTTPStatus: 201, Body: load("CreateBudget")},
		{Name: "canonical update budget", Args: []string{"update-budget", "--budget-uuid", "budget-1", "--status", "PAUSED"}, HTTPStatus: 200, Body: load("UpdateBudget")},
		{Name: "canonical delete budget", Args: []string{"delete-budget", "--budget-uuid", "budget-1", "--yes"}, HTTPStatus: 200, Body: load("DeleteBudget")},
		{Name: "canonical create threshold", Args: []string{"create-budget-threshold", "--budget-uuid", "budget-1", "--threshold-type", "ACTUAL", "--threshold-percentage", "100"}, HTTPStatus: 201, Body: load("CreateBudgetThreshold")},
		{Name: "canonical update threshold", Args: []string{"update-budget-threshold", "--budget-uuid", "budget-1", "--threshold-uuid", "threshold-1", "--enabled=false"}, HTTPStatus: 200, Body: load("UpdateBudgetThreshold")},
		{Name: "canonical delete threshold", Args: []string{"delete-budget-threshold", "--budget-uuid", "budget-1", "--threshold-uuid", "threshold-1", "--yes"}, HTTPStatus: 200, Body: load("DeleteBudgetThreshold")},
	}
	for i := range rows {
		row := &rows[i]
		t.Run(row.Name, func(t *testing.T) {
			fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){paths[row.Args[0]]: jsonHandler(row.HTTPStatus, row.Body)})
			root, stdout, _ := newSvcRoot(t, fixture)
			if row.Bearer != "" {
				withTestOptions(t, append(testOptions, vngcloud.WithStaticToken(row.Bearer))...)
			}
			root.SetArgs(append([]string{"--region", "hcm-3", "--output", "json", "billing"}, row.Args...))
			err := root.ExecuteContext(context.Background())
			if fixture.requestCount() != 1 {
				t.Fatal("expected one request")
			}
			if err == nil {
				row.Stdout = stdout.String()
				return
			}
			row.Exit = exitCode(err)
			diagnostic := classify(err)
			row.Code, row.Status = diagnostic.Code, diagnostic.Status
			if (row.HTTPStatus == 400 && row.Exit == 4) || row.Name == "empty envelope refusal message" || row.Name == "reflected refusal without duplicate keys" || row.Name == "short credential withholds the whole refusal" || row.Name == "refusal match budget withholds the whole field" {
				row.Message = diagnostic.Message
			}
		})
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(os.Getenv("BILLING_GOLDEN_DIR"), "validation-go-v1.8.0.json"), append(data, '\n'), 0o600); err != nil {
		t.Fatal(err)
	}
}
