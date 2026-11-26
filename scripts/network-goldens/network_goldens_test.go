package cli

import (
	"context"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"danny.vn/vngcloud"
)

func TestNetworkGoldens(t *testing.T) {
	for _, tc := range []struct {
		command, fixture, host, path string
		args                         []string
	}{
		{"get-nat-instance", "get_nat_instance.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/zone-1/proj-1/nats/nat-1", []string{"--nat-id", "nat-1", "--zone-id", "zone-1"}},
		{"list-nat-inbound-rules", "list_nat_inbound_rules.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/zone-1/proj-1/nats/nat-1/sec-groups/rules", []string{"--nat-id", "nat-1", "--zone-id", "zone-1"}},
		{"list-bandwidth-packages", "list_bandwidth_packages.json", "hcm-3.console.greennode.ai", "/vserver/bandwidth-api/v1/projects/proj-1/bandwidth/package-config", nil},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "network", tc.fixture))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				withCleanEnv(t)
				calls := 0
				transport := saCLITransport(func(r *http.Request) (*http.Response, error) {
					calls++
					if r.Method != "GET" || r.URL.Host != tc.host || r.URL.RequestURI() != tc.path {
						t.Fatal("unexpected network read route")
					}
					return &http.Response{StatusCode: http.StatusOK, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(string(body))), Request: r}, nil
				})
				withTestOptions(t, vngcloud.WithStaticToken("toy-token"), vngcloud.WithTransport(transport))
				var stdout, stderr strings.Builder
				root := newRootCmd(strings.NewReader(""), &stdout, &stderr)
				args := []string{"--region", "hcm-3", "--project-id", "proj-1", "--read-only", "--output", format, "network", tc.command}
				root.SetArgs(append(args, tc.args...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if calls != 1 || stderr.Len() != 0 {
					t.Fatal("expected one read and empty stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("NETWORK_GOLDEN_DIR"), tc.command+"."+format+".golden"), []byte(stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
