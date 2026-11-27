package cli

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"danny.vn/vngcloud/monitor"
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

func TestMonitorRemainingGoldens(t *testing.T) {
	type reply struct{ route, fixture string }
	cases := []struct {
		operation string
		flags     []string
		replies   []reply
	}{
		{"list-check-alarm-history", []string{"--check-id", "chk-1", "--start", "2000-01-01T07:00:00+07:00", "--end", "2000-01-05T00:00:00Z"}, []reply{{"/vmonitor-uptime-manager/v1/uptimes/chk-1", "check_alarm_snapshot"}, {"/vmonitor-api/api/v1/alarms/synthetic/metrics/mona/alarm-1", "check_alarm_definition"}, {"/vmonitor-api/api/v1/alarms/synthetic/metrics/mona/sub-1/histories", "remaining_history"}}},
		{"get-alarm", []string{"--alarm-id", "alarm-1"}, []reply{{"/vmonitor-api/api/v1/alarms/alarm-1", "remaining_alarm"}}},
		{"list-alarms", []string{"--kind", "Log"}, []reply{{"/vmonitor-api/api/v1/alarms/list", "remaining_alarms"}}},
		{"get-channel", []string{"--channel-id", "channel-1"}, []reply{{"/notification-gateway/api/v1/notification/list/typeSearch", "remaining_channels"}}},
		{"list-channels", []string{}, []reply{{"/notification-gateway/api/v1/notification/list/typeSearch", "remaining_channels"}}},
		{"list-channel-types", []string{}, []reply{{"/notification-gateway/api/v1/type/list", "remaining_channel_types"}}},
		{"get-check", []string{"--check-id", "chk-1"}, []reply{{"/vmonitor-uptime-manager/v1/uptimes/chk-1", "remaining_check"}}},
		{"list-checks", []string{}, []reply{{"/vmonitor-uptime-manager/v1/uptimes", "remaining_checks"}}},
		{"list-locations", []string{}, []reply{{"/vmonitor-uptime-manager/v1/locations", "remaining_locations"}}},
		{"get-log-project", []string{"--log-project-id", "project-1"}, []reply{{"/log-api/v1/projects/project-1", "remaining_log_project"}}},
		{"list-log-projects", []string{}, []reply{{"/log-api/v1/projects", "remaining_log_projects"}}},
		{"list-log-project-classes", []string{}, []reply{{"/billing-api/v2/log/quota-class", "remaining_log_classes"}}},
		{"quote-create-log-project", []string{"--name", "Logs"}, []reply{{"/billing-api/v2/log/quota-class", "remaining_log_classes"}, {"/billing-api/v2/log/prices/created-price", "remaining_log_quote"}}},
		{"get-quota-usage", []string{}, []reply{{"/billing-api/v1/quotas/usages", "remaining_quota_usage"}}},
		{"get-synthetic-quota", []string{}, []reply{{"/billing-api/v1/synthetic/quota", "quota_synthetic"}, {"/billing-api/v2/synthetic/quota-1/quota-detail", "quota_quota_detail"}}},
		{"get-synthetic-quota-usage", []string{}, []reply{{"/billing-api/v1/synthetic/quota/usages", "remaining_synthetic_usage"}}},
		{"list-notification-packages", []string{"--kind", "Email"}, []reply{{"/billing-api/v1/email/packages", "notification_quote_packages"}}},
	}
	for _, tc := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				routes := map[string]func(http.ResponseWriter, *http.Request){}
				for _, reply := range tc.replies {
					body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), reply.fixture+".json"))
					if err != nil {
						t.Fatal(err)
					}
					routes[reply.route] = jsonHandler(http.StatusOK, string(body))
				}
				server := newSvcFixture(routes)
				args := append([]string{"--profile", "agent", "--read-only", "--output", format, "monitor", tc.operation}, tc.flags...)
				code, stdout, stderr := dashboardCLI(t, server.mux)(args...)
				if code != 0 || stderr != "" || server.requestCount() != len(routes) {
					t.Fatalf("Monitor read failed (exit %d, requests %d)", code, server.requestCount())
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), tc.operation+"."+format+".golden"), []byte(stdout), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorChannelWriteGoldens(t *testing.T) {
	for _, tc := range []struct {
		operation, route, fixture string
		flags                     []string
	}{
		{"create-channel", "/notification-gateway/api/v1/notification", "created_channel", []string{"--name", "Alerts", "--type", "Webhook"}},
		{"update-channel", "/notification-gateway/api/v1/notification", "", []string{"--channel-id", "channel-1", "--name", "Changed"}},
		{"delete-channel", "/notification-gateway/api/v1/notification/channel-1", "", []string{"--channel-id", "channel-1", "--yes"}},
		{"send-channel-otp", "/notification-gateway/api/v1/notification/otps", "sent_channel_otp", []string{"--type", "Email", "--address", "example@example.test"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				load := func(name string) string {
					body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), name+".json"))
					if err != nil {
						t.Fatal(err)
					}
					return string(body)
				}
				body := ""
				if tc.fixture != "" {
					body = load(tc.fixture)
				}
				routes := map[string]func(http.ResponseWriter, *http.Request){tc.route: jsonHandler(200, body)}
				if tc.operation == "update-channel" {
					routes["/notification-gateway/api/v1/notification/list/typeSearch"] = jsonHandler(200, load("remaining_channels"))
				}
				server := newSvcFixture(routes)
				args := append([]string{"--region", "hcm-3", "--output", format, "monitor", tc.operation}, tc.flags...)
				if tc.operation == "create-channel" {
					path := filepath.Join(t.TempDir(), "channel.json")
					if err := os.WriteFile(path, []byte(`{"Address":"https://example.test/secret-path","Headers":[{"Key":"X-Hook","Value":"toy-hook"}]}`), 0600); err != nil {
						t.Fatal(err)
					}
					args = append(args, "--cli-input-json", "file://"+path)
				}
				root, stdout, stderr := newSvcRoot(t, server)
				root.SetArgs(args)
				if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 || server.requestCount() != len(routes) {
					t.Fatalf("Monitor channel write failed (requests %d)", server.requestCount())
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), tc.operation+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorCheckWriteGoldens(t *testing.T) {
	beforeBytes, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), "remaining_check.json"))
	if err != nil {
		t.Fatal(err)
	}
	before := string(beforeBytes)
	check := "/vmonitor-uptime-manager/v1/uptimes"
	for _, tc := range []struct {
		operation string
		flags     []string
	}{
		{"create-check", []string{"--name", "Probe", "--url", "https://example.test/", "--locations", "loc-1"}},
		{"update-check", []string{"--check-id", "chk-1", "--name", "Changed"}},
		{"delete-check", []string{"--check-id", "chk-1", "--yes"}},
		{"pause-check", []string{"--check-id", "chk-1"}},
		{"resume-check", []string{"--check-id", "chk-1"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				routes := map[string]func(http.ResponseWriter, *http.Request){}
				count := 1
				switch tc.operation {
				case "create-check":
					routes[check] = jsonHandler(201, before)
				case "update-check":
					count = 2
					routes[check+"/chk-1"] = func(w http.ResponseWriter, r *http.Request) {
						body := before
						if r.Method == "PUT" {
							body = strings.ReplaceAll(body, `"Probe"`, `"Changed"`)
						}
						jsonHandler(200, body)(w, r)
					}
				case "delete-check":
					routes[check+"/chk-1"] = jsonHandler(204, "")
				case "pause-check", "resume-check":
					count = 3
					var sent atomic.Bool
					routes[check+"/status/chk-1"] = func(w http.ResponseWriter, r *http.Request) { sent.Store(true); w.WriteHeader(204) }
					routes[check+"/chk-1"] = func(w http.ResponseWriter, r *http.Request) {
						body := before
						if (tc.operation == "pause-check") == sent.Load() {
							body = strings.ReplaceAll(body, "ENABLED", "DISABLED")
						}
						jsonHandler(200, body)(w, r)
					}
				}
				server := newSvcFixture(routes)
				root, stdout, stderr := newSvcRoot(t, server)
				root.SetArgs(append([]string{"--region", "hcm-3", "--output", format, "monitor", tc.operation}, tc.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 || server.requestCount() != count {
					t.Fatalf("Monitor check write failed (requests %d)", server.requestCount())
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), tc.operation+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorLogAlarmWriteGoldens(t *testing.T) {
	load := func(file string) string {
		body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), file+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	const prefix = "/vmonitor-api/api/v1/alarms"
	for _, tc := range []struct {
		operation, route string
		flags            []string
		status           int
		ack              string
	}{
		{"create-log-alarm", prefix + "/logs", []string{"--name", "Read alarm", "--log-project-id", "project-1", "--threshold-value", "0", "--no-wait"}, 201, `{"data":{"id":"alarm-1"}}`},
		{"update-log-alarm", prefix + "/logs/alarm-1", []string{"--alarm-id", "alarm-1", "--description", "Changed", "--no-wait"}, 200, ""},
		{"delete-log-alarm", prefix + "/logs/alarm-1", []string{"--alarm-id", "alarm-1", "--yes"}, 204, ""},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				routes := map[string]func(http.ResponseWriter, *http.Request){tc.route: jsonHandler(tc.status, tc.ack)}
				if tc.operation == "create-log-alarm" {
					routes[prefix+"/list"] = jsonHandler(200, `{"lstData":[],"page":1,"pageSize":10000,"totalItem":0,"totalPage":0}`)
					routes["/log-api/v1/projects/project-1"] = jsonHandler(200, load("remaining_log_project"))
				} else {
					routes[prefix+"/alarm-1"] = jsonHandler(200, load("log_alarm_guard"))
				}
				server := newSvcFixture(routes)
				root, stdout, stderr := newSvcRoot(t, server)
				root.SetArgs(append([]string{"--region", "hcm-3", "--output", format, "monitor", tc.operation}, tc.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 || server.requestCount() != len(routes) {
					t.Fatalf("log alarm write failed (requests %d)", server.requestCount())
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), tc.operation+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorLogProjectWriteGoldens(t *testing.T) {
	load := func(file string) string {
		body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), file+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	for _, op := range []string{"create-log-project", "delete-log-project"} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(op+"/"+format, func(t *testing.T) {
				args := []string{"--region", "hcm-3", "--output", format, "monitor", op}
				routes := map[string]func(http.ResponseWriter, *http.Request){}
				if op == "create-log-project" {
					args = append(args, "--name", "Logs", "--max-price", "5", "--no-wait")
					routes["/log-api/v1/projects"] = jsonHandler(200, `{"content":[],"currentPage":0,"pageSize":100,"totalPages":0,"totalElements":0}`)
					routes["/billing-api/v2/log/quota-class"] = jsonHandler(200, load("remaining_log_classes"))
					routes["/billing-api/v2/log/prices/created-price"] = jsonHandler(200, load("remaining_log_quote"))
					routes["/billing-api/v2/log/quotas"] = jsonHandler(201, `{"orderId":4001}`)
				} else {
					args = append(args, "--log-project-id", "project-1", "--purge", "--no-wait", "--yes")
					routes["/billing-api/v1/log/quotas/project-1"] = jsonHandler(404, `{}`)
					routes["/billing-api/v1/trash/log/quotas/project-1"] = jsonHandler(204, "")
				}
				server := newSvcFixture(routes)
				root, stdout, stderr := newSvcRoot(t, server)
				root.SetArgs(args)
				if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 || server.requestCount() != len(routes) {
					t.Fatalf("log project write failed (requests %d)", server.requestCount())
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), op+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorQuotaLifecycleWriteGoldens(t *testing.T) {
	load := func(file string) string {
		body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), file+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	for _, op := range []string{"create-notification-quota", "delete-notification-quota", "set-notification-quota-auto-renew", "delete-synthetic-quota", "set-synthetic-quota-auto-renew"} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(op+"/"+format, func(t *testing.T) {
				args := []string{"--region", "hcm-3", "--output", format, "monitor", op}
				routes := map[string]func(http.ResponseWriter, *http.Request){}
				const email = "/billing-api/v1/email/quota"
				const synth = "/billing-api/v1/synthetic/quota"
				var sent atomic.Bool
				count := 0
				switch op {
				case "create-notification-quota":
					args = append(args, "--kind", "Email", "--no-wait")
					count = 5
					routes[email] = func(w http.ResponseWriter, r *http.Request) {
						if r.Method == "POST" {
							jsonHandler(200, `{"orderId":"order-1","paymentUrl":null}`)(w, r)
						} else {
							jsonHandler(404, load("notification_quote_absent"))(w, r)
						}
					}
					routes["/billing-api/v1/email/packages"] = jsonHandler(200, load("notification_quote_packages"))
					routes["/billing-api/v2/email/prices/created-price"] = jsonHandler(200, `{"optimumPrice":0}`)
				case "delete-notification-quota":
					args = append(args, "--kind", "Email", "--yes", "--expected-quota-id", "quota-email")
					count = 4
					routes[email] = func(w http.ResponseWriter, r *http.Request) {
						if r.Method == "DELETE" {
							sent.Store(true)
							w.WriteHeader(204)
						} else if sent.Load() {
							jsonHandler(404, load("notification_quote_absent"))(w, r)
						} else {
							jsonHandler(200, load("notification_quota_email"))(w, r)
						}
					}
				case "set-notification-quota-auto-renew":
					args = append(args, "--kind", "Email", "--enabled=false")
					count = 4
					routes[email] = func(w http.ResponseWriter, r *http.Request) {
						body := load("notification_quota_email")
						if sent.Load() {
							body = strings.ReplaceAll(body, `"autoRenewPeriod": 1`, `"autoRenewPeriod": 0`)
						} else {
							body = strings.ReplaceAll(body, `"autoRenew": 0`, `"autoRenew": 1`)
						}
						jsonHandler(200, body)(w, r)
					}
					routes[email+"/quota-email/update-auto-renew"] = func(w http.ResponseWriter, _ *http.Request) { sent.Store(true); w.WriteHeader(200) }
				case "delete-synthetic-quota":
					args = append(args, "--expected-quota-id", "quota-1", "--yes")
					count = 6
					routes[synth] = func(w http.ResponseWriter, r *http.Request) {
						if r.Method == "DELETE" {
							sent.Store(true)
							w.WriteHeader(204)
						} else if sent.Load() {
							jsonHandler(404, load("synthetic_quote_absent"))(w, r)
						} else {
							jsonHandler(200, load("synthetic_quote_renewal"))(w, r)
						}
					}
					routes["/billing-api/v2/synthetic/quota-1/quota-detail"] = jsonHandler(200, load("quota_quota_detail"))
				case "set-synthetic-quota-auto-renew":
					args = append(args, "--enabled=true")
					count = 8
					routes[synth] = func(w http.ResponseWriter, r *http.Request) {
						body := load("synthetic_quote_renewal")
						if sent.Load() {
							body = strings.ReplaceAll(body, `"autoRenew": 0`, `"autoRenew": 1`)
						}
						jsonHandler(200, body)(w, r)
					}
					routes["/billing-api/v2/synthetic/prices/created-price"] = jsonHandler(200, `{"optimumPrice":0}`)
					routes["/billing-api/v2/synthetic/quota-1/quota-detail"] = jsonHandler(200, load("quota_quota_detail"))
					routes[synth+"/quota-1/update-auto-renew"] = func(w http.ResponseWriter, _ *http.Request) { sent.Store(true); w.WriteHeader(200) }
				}
				server := newSvcFixture(routes)
				_, stdout, stderr := newSvcRoot(t, server)
				if code := Main(context.Background(), args, strings.NewReader(""), stdout, stderr); code != 0 || server.requestCount() != count {
					t.Fatalf("quota lifecycle write failed (exit %d, requests %d)", code, server.requestCount())
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), op+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
				if stderr.Len() > 0 {
					if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), op+".stderr.golden"), stderr.Bytes(), 0600); err != nil {
						t.Fatal(err)
					}
				}
			})
		}
	}
}

func TestMonitorSyntheticOrderWriteGoldens(t *testing.T) {
	load := func(file string) string {
		body, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), file+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	const quota = "/billing-api/v1/synthetic/quota"
	const prices = "/billing-api/v2/synthetic/prices/created-price"
	const classes = "/billing-api/v2/synthetic/quota-class"
	for _, tc := range []struct {
		op       string
		flags    []string
		mutation string
	}{
		{"create-synthetic-quota", []string{"--class-id", "class-1", "--package-id", "unit_1", "--quantity", "10"}, "/billing-api/v2/synthetic/quota"},
		{"renew-synthetic-quota", []string{"--expected-end-time", "2099-02-01T00:00:00Z"}, "/billing-api/v2/synthetic/quota/extend-time"},
		{"recover-synthetic-quota", []string{"--expected-quota-id", "quota-1"}, "/billing-api/v2/trash/synthetic/quota"},
		{"ensure-synthetic-quota", []string{"--expected-quota-id", "quota-1", "--max-days-left", "0"}, "/billing-api/v2/synthetic/quota/extend-time"},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.op+"/"+format, func(t *testing.T) {
				routes := map[string]func(http.ResponseWriter, *http.Request){
					classes:                       jsonHandler(200, load("quota_classes")),
					classes + "/class-1/packages": jsonHandler(200, load("quota_class_packages")),
					classes + "/class-2/packages": jsonHandler(200, "[]"),
					"/billing-api/v1/settings":    jsonHandler(200, load("quota_settings")),
					prices:                        jsonHandler(200, `{"optimumPrice":0}`),
					tc.mutation:                   jsonHandler(200, `{"orderId":"synth-order","paymentUrl":null}`),
				}
				count := 14
				selected := format
				if tc.op == "create-synthetic-quota" {
					count = 12
					selected = "json"
					routes[quota] = jsonHandler(404, load("synthetic_quote_absent"))
				} else {
					before := load("synthetic_quote_renewal")
					if tc.op == "recover-synthetic-quota" {
						before = strings.ReplaceAll(strings.ReplaceAll(before, "ACTIVE", "IN-TRASH"), "2099-", "2000-")
					}
					routes[quota] = jsonHandler(200, before)
					routes["/billing-api/v2/synthetic/quota-1/quota-detail"] = jsonHandler(200, load("quota_quota_detail"))
				}
				server := newSvcFixture(routes)
				_, stdout, stderr := newSvcRoot(t, server)
				args := append([]string{"--region", "hcm-3", "--output", selected, "monitor", tc.op, "--max-price", "0", "--no-wait"}, tc.flags...)
				if code := Main(context.Background(), args, strings.NewReader(""), stdout, stderr); code != 0 || stderr.Len() != 0 || server.requestCount() != count {
					t.Fatalf("synthetic quota write failed (exit %d, requests %d)", code, server.requestCount())
				}
				if tc.op == "create-synthetic-quota" {
					var output monitor.CreateSyntheticQuotaOutput
					if err := json.Unmarshal(stdout.Bytes(), &output); err != nil {
						t.Fatal(err)
					}
					fixed := time.Date(2000, 1, 1, 0, 0, 0, 0, time.UTC)
					output.SentAt = &fixed
					stdout.Reset()
					if err := renderOutput(stdout, format, "", &output, true); err != nil {
						t.Fatal(err)
					}
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), tc.op+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestMonitorSyntheticConfirmationGolden(t *testing.T) {
	load := func(name string) string {
		data, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(data)
	}
	const classes = "/billing-api/v2/synthetic/quota-class"
	current := load("synthetic_quote_renewal")
	extended := strings.ReplaceAll(current, "2099-02-01", "2099-03-01")
	var reads atomic.Int32
	server := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
		classes:                                          jsonHandler(200, load("quota_classes")),
		classes + "/class-1/packages":                    jsonHandler(200, load("quota_class_packages")),
		classes + "/class-2/packages":                    jsonHandler(200, "[]"),
		"/billing-api/v1/settings":                       jsonHandler(200, load("quota_settings")),
		"/billing-api/v2/synthetic/prices/created-price": jsonHandler(200, `{"optimumPrice":0}`),
		"/billing-api/v2/synthetic/quota/extend-time":    jsonHandler(200, `{"orderId":"synth-order"}`),
		"/billing-api/v2/synthetic/quota-1/quota-detail": jsonHandler(200, load("quota_quota_detail")),
		"/billing-api/v1/synthetic/quota": func(w http.ResponseWriter, r *http.Request) {
			reply := current
			if reads.Add(1) == 3 {
				reply = extended
			}
			jsonHandler(200, reply)(w, r)
		},
	})
	_, stdout, stderr := newSvcRoot(t, server)
	args := []string{"--region", "hcm-3", "--output", "json", "monitor", "ensure-synthetic-quota", "--max-price", "0", "--max-days-left", "0"}
	if code := Main(context.Background(), args, strings.NewReader(""), stdout, stderr); code != 0 || stderr.Len() != 0 || server.requestCount() != 16 {
		t.Fatalf("synthetic confirmation failed (exit %d, requests %d)", code, server.requestCount())
	}
	if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), "ensure-synthetic-quota-confirmed.json.golden"), stdout.Bytes(), 0600); err != nil {
		t.Fatal(err)
	}
}

func TestMonitorQuotaWriteErrorGoldens(t *testing.T) {
	load := func(name string) string {
		data, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(data)
	}
	const email = "/billing-api/v1/email/quota"
	for _, tc := range []struct {
		name   string
		args   []string
		routes map[string]func(http.ResponseWriter, *http.Request)
		count  int
	}{
		{"notification-quota-delete-refused", []string{"delete-notification-quota", "--kind", "Email", "--yes"}, map[string]func(http.ResponseWriter, *http.Request){email: func(w http.ResponseWriter, r *http.Request) {
			if r.Method == "DELETE" {
				jsonHandler(409, `{"message":"ignored"}`)(w, r)
			} else {
				jsonHandler(200, load("notification_quota_email"))(w, r)
			}
		}}, 3},
		{"notification-quota-price-refused", []string{"create-notification-quota", "--kind", "Email"}, map[string]func(http.ResponseWriter, *http.Request){email: jsonHandler(404, load("notification_quote_absent")), "/billing-api/v1/email/packages": jsonHandler(200, load("notification_quote_packages")), "/billing-api/v2/email/prices/created-price": jsonHandler(200, `{"optimumPrice":1}`)}, 3},
	} {
		t.Run(tc.name, func(t *testing.T) {
			server := newSvcFixture(tc.routes)
			_, stdout, stderr := newSvcRoot(t, server)
			args := append([]string{"--region", "hcm-3", "--output", "json", "monitor"}, tc.args...)
			if code := Main(context.Background(), args, strings.NewReader(""), stdout, stderr); code != 1 || server.requestCount() != tc.count {
				t.Fatalf("quota refusal mismatch (exit %d, requests %d)", code, server.requestCount())
			}
			if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), tc.name+".stderr.golden"), stderr.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		})
	}
}
