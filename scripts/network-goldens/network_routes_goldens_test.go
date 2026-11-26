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

func TestNetworkRoutesGoldens(t *testing.T) {
	type response struct{ path, fixture string }
	for _, tc := range []struct {
		command, host, gateway string
		args                   []string
		responses              []response
	}{
		{"get-route-table", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--route-table-id", "rt-2"}, []response{{"/v2/proj-1/route-table/rt-2", "get_route_table.json"}}},
		{"get-virtual-ip-address", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--virtual-ip-address-id", "vip-1"}, []response{{"/v2/proj-1/virtualIpAddress/vip-1", "get_virtual_ip_address.json"}}},
		{"list-address-pairs-by-virtual-ip-address", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--virtual-ip-address-id", "vip-1"}, []response{{"/v2/proj-1/virtualIpAddress/vip-1/addressPairs", "list_address_pairs_by_virtual_ip_address.json"}}},
		{"list-address-pairs-by-virtual-subnet", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--virtual-subnet-id", "subnet-1"}, []response{{"/v2/proj-1/virtual-subnets/subnet-1/addressPairs", "list_virtual_subnet_address_pairs.json"}}},
		{"list-all-virtual-ip-address-address-pairs", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{}, []response{{"/v2/proj-1/virtualIpAddress?name=&page=1&size=10000", "list_virtual_ip_addresses.json"}, {"/v2/proj-1/virtualIpAddress/vip-1/addressPairs", "list_address_pairs_by_virtual_ip_address.json"}}},
		{"list-interconnects", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--name", "test & sample", "--page", "2", "--size", "5"}, []response{{"/v2/proj-1/interconnects?name=test+%26+sample&page=2&size=5", "list_interconnects.json"}}},
		{"list-network-interfaces", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--name", "test & sample", "--page", "2", "--size", "5"}, []response{{"/v2/proj-1/network-interfaces-elastic?name=test+%26+sample&page=2&size=5", "list_network_interfaces.json"}}},
		{"list-peerings", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--name", "test & sample", "--page", "2", "--size", "5"}, []response{{"/v2/proj-1/peering?name=test+%26+sample&page=2&size=5", "list_peerings.json"}}},
		{"list-route-table-routes", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{}, []response{{"/v2/proj-1/route-table?name=&page=1&size=10000", "list_route_tables.json"}}},
		{"list-route-tables", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--name", "test & sample", "--page", "2", "--size", "5"}, []response{{"/v2/proj-1/route-table?name=test+%26+sample&page=2&size=5", "list_route_tables.json"}}},
		{"list-virtual-ip-addresses", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--name", "test & sample", "--page", "2", "--size", "5"}, []response{{"/v2/proj-1/virtualIpAddress?name=test+%26+sample&page=2&size=5", "list_virtual_ip_addresses.json"}}},
		{"list-vpn-connections", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway", []string{"--page", "2", "--size", "5"}, []response{{"/vnetwork/v1/proj-1/vpns?params=%7B%22search%22%3A%5B%7B%22field%22%3A%22any%22%2C%22value%22%3A%22%22%7D%5D%2C%22sort%22%3A%7B%7D%2C%22page%22%3A2%2C%22size%22%3A5%7D", "list_vpn_connections.json"}}},
		{"list-wanips", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway", []string{"--name", "test & sample", "--page", "2", "--size", "5"}, []response{{"/v2/proj-1/wanIps?name=test+%26+sample&page=2&size=5", "list_wan_ips.json"}}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				withCleanEnv(t)
				calls := 0
				transport := saCLITransport(func(r *http.Request) (*http.Response, error) {
					if calls == len(tc.responses) {
						t.Fatal("unexpected extra request")
					}
					next := tc.responses[calls]
					calls++
					if r.Method != "GET" || r.URL.Host != tc.host || r.URL.RequestURI() != tc.gateway+next.path {
						t.Fatalf("unexpected network request for %s: %s %s", tc.command, r.Method, r.URL)
					}
					body, err := os.ReadFile(filepath.Join(os.Getenv("NETWORK_ROUTES_FIXTURE_DIR"), next.fixture))
					if err != nil {
						t.Fatal(err)
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
				if calls != len(tc.responses) || stderr.Len() != 0 {
					t.Fatal("unexpected requests or stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("NETWORK_GOLDEN_DIR"), tc.command+"."+format+".golden"), []byte(stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
