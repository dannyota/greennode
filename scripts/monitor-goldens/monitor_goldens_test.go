package cli

import (
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"testing"
)

func TestMonitorReadGoldens(t *testing.T) {
	fixture := func(name string) string {
		t.Helper()
		body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	const dashboards = "/vmonitor-api/api/v1/dashboards"
	const check = "/vmonitor-uptime-manager/v1/uptimes/chk-1"
	const statistics = "/vmonitor-api/api/v1/statistics/synthetics"
	cases := []struct {
		operation string
		flags     []string
		fixtures  map[string]string
	}{
		{"get-dashboard", []string{"--dashboard-id", "dash-1"}, map[string]string{dashboards + "/dash-1": "get_dashboard_system"}},
		{"list-dashboards", []string{"--system"}, map[string]string{dashboards: "list_dashboards_system"}},
		{"list-check-response-times", []string{"--check-id", "chk-1", "--start", "2000-01-01T00:00:00Z", "--end", "2000-01-02T00:00:00Z"}, map[string]string{check: "response_time_check", statistics: "ListCheckResponseTimes"}},
	}
	for _, tc := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				routes := make(map[string]func(http.ResponseWriter, *http.Request))
				for path, file := range tc.fixtures {
					routes[path] = jsonHandler(http.StatusOK, fixture(file))
				}
				server := newSvcFixture(routes)
				run := dashboardCLI(t, server.mux)
				args := append([]string{"--profile", "agent", "--read-only", "--output", format, "monitor", tc.operation}, tc.flags...)
				code, stdout, stderr := run(args...)
				if code != 0 || stderr != "" || server.requestCount() != len(routes) {
					t.Fatalf("Monitor read failed (exit %d, requests %d)", code, server.requestCount())
				}
				name := tc.operation + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), name), []byte(stdout), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorQuotaGoldens(t *testing.T) {
	for _, operation := range []string{"list-synthetic-packages", "quota-status", "quota-status-partial"} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(operation+"/"+format, func(t *testing.T) {
				routes := map[string]func(http.ResponseWriter, *http.Request){}
				args := []string{"--profile", "agent", "--read-only", "--output", format, "monitor", operation}
				wantExit := 0
				if operation == "list-synthetic-packages" {
					for route, name := range map[string]string{
						"/billing-api/v2/synthetic/quota-class":                  "quota_classes",
						"/billing-api/v1/synthetic/packages":                     "quota_v1_packages",
						"/billing-api/v1/settings":                               "quota_settings",
						"/billing-api/v2/synthetic/quota-class/class-1/packages": "quota_class_packages",
						"/billing-api/v2/synthetic/quota-class/class-2/packages": "quota_empty_packages",
					} {
						body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), name+".json"))
						if err != nil {
							t.Fatal(err)
						}
						routes[route] = jsonHandler(http.StatusOK, string(body))
					}
				} else {
					for _, kind := range []string{"synthetic", "email", "sms"} {
						routes["/billing-api/v1/"+kind+"/quota"] = jsonHandler(http.StatusNotFound, "{}")
					}
					if operation == "quota-status-partial" {
						wantExit = 1
						routes["/billing-api/v1/email/quota"] = jsonHandler(http.StatusForbidden, `{"message":"peer-canary"}`)
						args = append(args[:len(args)-1], "quota-status", "--query", "Items[0].Kind")
					}
				}
				server := newSvcFixture(routes)
				code, stdout, stderr := dashboardCLI(t, server.mux)(args...)
				if code != wantExit || server.requestCount() != len(routes) {
					t.Fatalf("Monitor quota read failed (exit %d, requests %d)", code, server.requestCount())
				}
				if operation != "list-synthetic-packages" {
					stdout = regexp.MustCompile(`"AsOf": "[^"]+"`).ReplaceAllString(stdout, `"AsOf": "2000-01-01T00:00:00Z"`)
					stdout = regexp.MustCompile(`(?m)^As of [^;]+;`).ReplaceAllString(stdout, `As of 2000-01-01 00:00:00 UTC;`)
				}
				write := func(name, text string) {
					if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), name), []byte(text), 0600); err != nil {
						t.Fatal(err)
					}
				}
				write(operation+"."+format+".golden", stdout)
				if wantExit != 0 {
					write(operation+"."+format+".stderr.golden", stderr)
				}
			})
		}
	}
}

func TestMonitorSyntheticQuoteGoldens(t *testing.T) {
	fixture := func(name string) string {
		body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	for _, tc := range []struct {
		operation string
		flags     []string
		renew     bool
	}{
		{"quote-create-synthetic-quota", []string{"--class-id", "class-1", "--package-id", "unit_1", "--quantity", "10"}, false},
		{"quote-renew-synthetic-quota", []string{"--expected-end-time", "2099-02-01T00:00:00Z"}, true},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				routes := map[string]func(http.ResponseWriter, *http.Request){
					"/billing-api/v2/synthetic/quota-class":                  jsonHandler(200, fixture("quota_classes")),
					"/billing-api/v2/synthetic/quota-class/class-1/packages": jsonHandler(200, fixture("quota_class_packages")),
					"/billing-api/v2/synthetic/quota-class/class-2/packages": jsonHandler(200, "[]"),
					"/billing-api/v1/settings":                               jsonHandler(200, fixture("quota_settings")),
					"/billing-api/v2/synthetic/prices/created-price":         jsonHandler(200, fixture("synthetic_quote_price")),
					"/billing-api/v1/synthetic/quota":                        jsonHandler(404, fixture("synthetic_quote_absent")),
				}
				if tc.renew {
					routes["/billing-api/v1/synthetic/quota"] = jsonHandler(200, fixture("synthetic_quote_renewal"))
					routes["/billing-api/v2/synthetic/quota-1/quota-detail"] = jsonHandler(200, fixture("quota_quota_detail"))
				}
				server := newSvcFixture(routes)
				args := append([]string{"--profile", "agent", "--read-only", "--output", format, "monitor", tc.operation}, tc.flags...)
				code, stdout, stderr := dashboardCLI(t, server.mux)(args...)
				if code != 0 || stderr != "" || server.requestCount() != len(routes) {
					t.Fatalf("Monitor quote failed (exit %d, requests %d)", code, server.requestCount())
				}
				name := tc.operation + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), name), []byte(stdout), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorNotificationGoldens(t *testing.T) {
	for _, tc := range []struct{ operation, route, fixture string }{
		{"get-notification-quota", "/billing-api/v1/email/quota", "notification_quota_email"},
		{"get-notification-quota-usage", "/billing-api/v1/email/quota/usages", "notification_usage_email"},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), tc.fixture+".json"))
				if err != nil {
					t.Fatal(err)
				}
				server := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){tc.route: jsonHandler(200, string(body))})
				code, stdout, stderr := dashboardCLI(t, server.mux)("--profile", "agent", "--read-only", "--output", format, "monitor", tc.operation, "--kind", "Email")
				if code != 0 || stderr != "" || server.requestCount() != 1 {
					t.Fatalf("Monitor notification read failed (exit %d, requests %d)", code, server.requestCount())
				}
				name := tc.operation + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), name), []byte(stdout), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorNextReadGoldens(t *testing.T) {
	type step struct {
		route, fixture string
		status         int
	}
	for _, tc := range []struct {
		operation string
		flags     []string
		steps     []step
	}{
		{"quote-create-notification-quota", []string{"--kind", "Email", "--package-id", "emailplus"}, []step{
			{"/billing-api/v1/email/quota", "notification_quote_absent", 404},
			{"/billing-api/v1/email/packages", "notification_quote_packages", 200},
			{"/billing-api/v2/email/prices/created-price", "notification_quote_paid", 200},
		}},
		{"get-check-alarm", []string{"--check-id", "chk-1"}, []step{
			{"/vmonitor-uptime-manager/v1/uptimes/chk-1", "check_alarm_snapshot", 200},
			{"/vmonitor-api/api/v1/alarms/synthetic/metrics/mona/alarm-1", "check_alarm_definition", 200},
		}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				routes := map[string]func(http.ResponseWriter, *http.Request){}
				for _, step := range tc.steps {
					body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), step.fixture+".json"))
					if err != nil {
						t.Fatal(err)
					}
					routes[step.route] = jsonHandler(step.status, string(body))
				}
				server := newSvcFixture(routes)
				args := append([]string{"--profile", "agent", "--read-only", "--output", format, "monitor", tc.operation}, tc.flags...)
				code, stdout, stderr := dashboardCLI(t, server.mux)(args...)
				if code != 0 || stderr != "" || server.requestCount() != len(routes) {
					t.Fatalf("Monitor read failed (exit %d, requests %d)", code, server.requestCount())
				}
				name := tc.operation + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), name), []byte(stdout), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
