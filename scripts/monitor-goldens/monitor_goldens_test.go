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
