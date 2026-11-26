package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync/atomic"
	"testing"
)

func TestCDNGoldens(t *testing.T) {
	domains := []string{"--cli-input-json", `{"CDNDomains":["cdn.example.test"]}`, "--period", "24h"}
	cases := []struct {
		operation string
		flags     []string
	}{
		{"get-cache-status", domains},
		{"get-certificate", []string{"--certificate-id", "cert-1"}},
		{"get-http-codes", domains},
		{"get-request-rate", domains},
		{"get-traffic", domains},
		{"get-traffic-report", []string{"--cli-input-json", `{"CDNDomains":["cdn.example.test"]}`, "--from", "2026-10-08", "--to", "2026-10-09"}},
		{"get-web-accelerator", []string{"--cdn-id", "cdn-1"}},
		{"list-api-keys", nil},
		{"list-certificates", nil},
		{"list-ip-ranges", nil},
		{"list-web-accelerators", nil},
	}
	for _, tc := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				var output string
				args := append([]string{"--output", format, "cdn", tc.operation}, tc.flags...)
				if tc.operation == "list-ip-ranges" {
					fixture, _ := cdnFixtureServer(t, "../../testdata/cdn/faq-vcdn.html", "text/html")
					root, stdout, stderr := newSvcRoot(t, fixture)
					root.SetArgs(append([]string{"--region", "hcm-3"}, args...))
					if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 || fixture.requestCount() != 1 {
						t.Fatal("public range read failed")
					}
					output = regexp.MustCompile(`https?://127\.0\.0\.1:\d+/faq/vcdn`).ReplaceAllString(stdout.String(), "https://docs.greennode.ai/faq/vcdn")
				} else {
					fixture := newVCDNServer(t)
					stdout, stderr, run := vcdnRoot(t, fixture, vcdnKeyPlaceholder)
					if err := run(args...); err != nil || stderr.Len() != 0 || fixture.requestCount() != 1 {
						t.Fatal("vCDN read failed")
					}
					output = stdout.String()
				}
				if strings.Contains(output, vcdnKeyPlaceholder) {
					t.Fatal("retained the vCDN key")
				}
				name := tc.operation + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("CDN_GOLDEN_DIR"), name), []byte(output), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestCDNWriteGoldens(t *testing.T) {
	cases := []struct {
		operation            string
		flags                []string
		route                string
		before, write, after string
	}{
		{"update-web-accelerator", []string{"--cdn-id", "cdn-1", "--origin-host-header", "new.example.test", "--no-wait"}, "/v1/cdn/update", "webaccelerator-detail.json", "write-update.json", "webaccelerator-detail-deploying.json"},
		{"delete-web-accelerator", []string{"--cdn-id", "cdn-1", "--yes"}, "/v1/cdn/delete/cdn-1", "webaccelerator-detail-disabled.json", "write-toggle.json", ""},
		{"enable-web-accelerator", []string{"--cdn-id", "cdn-1", "--no-wait"}, "/v1/cdn/status/change/cdn-1", "webaccelerator-detail-disabled.json", "write-toggle.json", "webaccelerator-detail-deploying.json"},
		{"disable-web-accelerator", []string{"--cdn-id", "cdn-1", "--yes"}, "/v1/cdn/status/change/cdn-1", "webaccelerator-detail.json", "write-toggle.json", "webaccelerator-detail-disabled.json"},
		{"purge-paths", []string{"--cdn-domain", "cdn.example.test", "--cli-input-json", `{"Paths":["/index.html"]}`}, "/v1/cdn/flush-cache", "", "purge-success.json", ""},
	}
	for _, tc := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.operation+"/"+format, func(t *testing.T) {
				handler := func(file string) http.HandlerFunc {
					body, err := os.ReadFile("../../testdata/cdn/" + file)
					if err != nil {
						t.Fatal(err)
					}
					return jsonHandler(http.StatusOK, string(body))
				}
				routes := map[string]func(http.ResponseWriter, *http.Request){tc.route: handler(tc.write)}
				count := 1
				if tc.before != "" {
					count++
					read := handler(tc.before)
					if tc.after == "" {
						routes["/v1/cdn/detail/cdn-1"] = read
					} else {
						count++
						after := handler(tc.after)
						var seen atomic.Bool
						routes["/v1/cdn/detail/cdn-1"] = func(w http.ResponseWriter, r *http.Request) {
							if seen.Swap(true) {
								after(w, r)
							} else {
								read(w, r)
							}
						}
					}
				}
				fixture := &vcdnServer{svcFixture: newSvcFixture(routes)}
				stdout, stderr, run := vcdnRoot(t, fixture, vcdnKeyPlaceholder)
				args := append([]string{"--output", format, "cdn", tc.operation}, tc.flags...)
				if err := run(args...); err != nil || stderr.Len() != 0 || fixture.requestCount() != count {
					t.Fatal("vCDN write failed")
				}
				if strings.Contains(stdout.String(), vcdnKeyPlaceholder) {
					t.Fatal("retained the vCDN key")
				}
				name := tc.operation + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("CDN_GOLDEN_DIR"), name), []byte(stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
