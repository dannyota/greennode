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

func TestNetworkVPCGoldens(t *testing.T) {
	type read struct{ path, fixture string }
	for _, tc := range []struct {
		command string
		args    []string
		reads   []read
	}{
		{"get-dhcp-options", []string{"--dhcp-options-id", "dop-1"}, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/dhcp_option/dop-1", "get_dhcp_options.json"}}},
		{"get-network-acl", []string{"--network-acl-id", "acl-1"}, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/network-acl/acl-1", "get_network_acl.json"}}},
		{"get-security-group", []string{"--security-group-id", "sg-1"}, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/secgroups/sg-1", "get_security_group.json"}}},
		{"get-subnet", []string{"--vpc-id", "vpc-1", "--subnet-id", "subnet-1"}, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/networks/vpc-1/subnets/subnet-1", "get_subnet.json"}}},
		{"get-vpc", []string{"--vpc-id", "vpc-1"}, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/networks/vpc-1", "get_vpc.json"}}},
		{"list-all-security-group-rules", nil, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/secgroups?name=&page=1&size=10000", "list_security_groups.json"}, {"/vserver/iam-vserver-gateway/v2/proj-1/secgroups/sg-1/secGroupRules", "list_security_group_rules.json"}}},
		{"list-dhcp-options", nil, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/dhcp_option?name=&page=1&size=10000", "list_dhcp_options.json"}}},
		{"list-network-acls", nil, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/network-acl/list?name=&page=1&size=10000", "list_network_acls.json"}}},
		{"list-security-group-rules", []string{"--security-group-id", "sg-1"}, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/secgroups/sg-1/secGroupRules", "list_security_group_rules.json"}}},
		{"list-security-groups", nil, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/secgroups?name=&page=1&size=10000", "list_security_groups.json"}}},
		{"list-subnets", nil, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/networks?name=&page=1&size=10000", "list_vpcs.json"}, {"/vserver/iam-vserver-gateway/v2/proj-1/networks/vpc-1/subnets", "list_subnets_by_vpc.json"}}},
		{"list-subnets-by-vpc", []string{"--vpc-id", "vpc-1"}, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/networks/vpc-1/subnets", "list_subnets_by_vpc.json"}}},
		{"list-vpcs", nil, []read{{"/vserver/iam-vserver-gateway/v2/proj-1/networks?name=&page=1&size=10000", "list_vpcs.json"}}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				withCleanEnv(t)
				calls := 0
				transport := saCLITransport(func(r *http.Request) (*http.Response, error) {
					if calls >= len(tc.reads) {
						t.Fatal("unexpected inventory request")
					}
					read := tc.reads[calls]
					calls++
					if r.Method != "GET" || r.URL.Host != "hcm-3.console.greennode.ai" || r.URL.RequestURI() != read.path {
						t.Fatalf("unexpected inventory route: %s %s", r.Method, r.URL.RequestURI())
					}
					body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "network", read.fixture))
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
				if calls != len(tc.reads) || stderr.Len() != 0 {
					t.Fatal("expected all reads and empty stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("NETWORK_GOLDEN_DIR"), tc.command+"."+format+".golden"), []byte(stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
