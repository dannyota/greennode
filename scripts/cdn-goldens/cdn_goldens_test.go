package cli

import (
	"context"
	"os"
	"path/filepath"
	"regexp"
	"strings"
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
