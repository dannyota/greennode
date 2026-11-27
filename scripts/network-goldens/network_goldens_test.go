package cli

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
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

func TestNetworkInventoryGoldens(t *testing.T) {
	for _, tc := range []struct {
		command, fixture, host, path, query string
		args                                []string
		prereq                              string
	}{
		{"get-endpoint", "get_endpoint.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/zone-a/proj-1/endpoints/endpoint-1", "", []string{"--endpoint-id", "endpoint-1"}, "regions"},
		{"list-endpoint-tags", "list_endpoint_tags.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/proj-1/tags", "resourceUuid=endpoint-1", []string{"--endpoint-id", "endpoint-1"}, "projects"},
		{"list-endpoints", "list_endpoints.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/zone-a/proj-1/endpoints", url.Values{"params": {`{"page":2,"search":[{"field":"vpcId","value":"vpc-1"},{"field":"uuid","value":"endpoint-1"}],"size":25}`}}.Encode(), []string{"--zone-id", "zone-a", "--vpc-id", "vpc-1", "--uuid", "endpoint-1", "--page", "2", "--size", "25"}, ""},
		{"list-nat-instances", "list_nat_instances.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/zone-1/proj-1/nats", url.Values{"params": {`{"search":[],"sort":{},"page":2,"size":5}`}}.Encode(), []string{"--zone-id", "zone-1", "--page", "2", "--size", "5"}, ""},
		{"list-nat-zones", "list_nat_zones.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/zone-1/proj-1/nats/zones", url.Values{"params": {`{"search":[],"sort":{},"page":1,"size":1000}`}}.Encode(), []string{"--zone-id", "zone-1"}, ""},
		{"list-nat-packages", "list_nat_packages.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/zone-1/proj-1/nats/nat-package", url.Values{"params": {`{"search":[],"sort":{},"page":1,"size":1000}`}, "zoneUuid": {"az-1"}}.Encode(), []string{"--zone-id", "zone-1", "--availability-zone-id", "az-1"}, ""},
		{"list-v-network-regions", "list_vnetwork_regions.json", "hcm-3-vnetwork.console.greennode.ai", "/vnetwork-gateway/vnetwork/v1/regions", "", nil, ""},
		{"list-servers-by-security-group", "list_servers_by_security_group.json", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway/v2/proj-1/secgroups/group-1/servers", "", []string{"--security-group-id", "group-1"}, ""},
		{"list-servers-by-subnet", "list_servers_by_subnet.json", "hcm-3.console.greennode.ai", "/vserver/iam-vserver-gateway/v2/proj-1/servers/subnets/subnet-1", "", []string{"--subnet-id", "subnet-1"}, ""},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "network", tc.fixture))
		if err != nil {
			t.Fatal(err)
		}
		regions, err := os.ReadFile(filepath.Join("..", "..", "testdata", "network", "list_vnetwork_regions.json"))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				withCleanEnv(t)
				calls := 0
				transport := saCLITransport(func(r *http.Request) (*http.Response, error) {
					calls++
					data := string(body)
					switch {
					case calls == 1 && tc.prereq == "regions":
						if r.URL.Host != tc.host || r.URL.RequestURI() != "/vnetwork-gateway/vnetwork/v1/regions" {
							t.Fatal("unexpected region discovery route")
						}
						data = string(regions)
					case calls == 1 && tc.prereq == "projects":
						if r.URL.Host != "hcm-3.console.greennode.ai" || r.URL.RequestURI() != "/vserver/iam-vserver-gateway/v1/projects" {
							t.Fatal("unexpected project metadata route")
						}
						data = `{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":123}]}`
					default:
						if r.URL.Host != tc.host || r.URL.Path != tc.path || r.URL.RawQuery != tc.query || (tc.prereq == "projects" && r.Header.Get("portal-user-id") != "123") {
							t.Fatalf("unexpected inventory route %s", r.URL.RequestURI())
						}
					}
					if r.Method != http.MethodGet {
						t.Fatal("expected GET")
					}
					return &http.Response{StatusCode: http.StatusOK, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(data)), Request: r}, nil
				})
				withTestOptions(t, vngcloud.WithStaticToken("toy-token"), vngcloud.WithTransport(transport))
				var stdout, stderr strings.Builder
				root := newRootCmd(strings.NewReader(""), &stdout, &stderr)
				args := []string{"--region", "hcm-3", "--project-id", "proj-1", "--read-only", "--output", format, "network", tc.command}
				root.SetArgs(append(args, tc.args...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				expected := 1
				if tc.prereq != "" {
					expected++
				}
				if calls != expected || stderr.Len() != 0 {
					t.Fatal("unexpected call count or stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("NETWORK_GOLDEN_DIR"), tc.command+"."+format+".golden"), []byte(stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestNetworkPriceGoldens(t *testing.T) {
	fixture, err := os.ReadFile(filepath.Join("..", "..", "testdata", "network", "discovery_quote_public_vm.json"))
	if err != nil {
		t.Fatal(err)
	}
	for _, format := range []string{"json", "table", "text"} {
		t.Run(format, func(t *testing.T) {
			withCleanEnv(t)
			calls := 0
			transport := saCLITransport(func(r *http.Request) (*http.Response, error) {
				if r.Method != http.MethodPost || r.URL.Host != "hcm-3.console.greennode.ai" || r.URL.RequestURI() != "/vserver/iam-billing-gateway/v1/price" {
					t.Fatal("unexpected quote route")
				}
				var req struct {
					ResourceInfo struct {
						Type string `json:"type"`
					} `json:"resourceInfo"`
				}
				if json.NewDecoder(r.Body).Decode(&req) != nil || calls >= 3 || req.ResourceInfo.Type != []string{"public", "public-vm", "public-mkp"}[calls] {
					t.Fatalf("unexpected public IP quote selector")
				}
				calls++
				return &http.Response{StatusCode: http.StatusOK, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(string(fixture))), Request: r}, nil
			})
			withTestOptions(t, vngcloud.WithStaticToken("toy-token"), vngcloud.WithTransport(transport))
			var out, stderr strings.Builder
			root := newRootCmd(strings.NewReader(""), &out, &stderr)
			root.SetArgs([]string{"--region", "hcm-3", "--project-id", "proj-1", "--read-only", "--output", format, "network", "list-network-prices"})
			if err := root.ExecuteContext(context.Background()); err != nil {
				t.Fatal(err)
			}
			if calls != 3 {
				t.Fatalf("expected three quote requests, got %d", calls)
			}
			stdout := regexp.MustCompile(`("QuotedAt": ")[^"]+(")`).ReplaceAllString(out.String(), `${1}<time>${2}`)
			for suffix, bytes := range map[string]string{"": stdout, ".stderr": stderr.String()} {
				if err := os.WriteFile(filepath.Join(os.Getenv("NETWORK_GOLDEN_DIR"), "list-network-prices."+format+suffix+".golden"), []byte(bytes), 0600); err != nil {
					t.Fatal(err)
				}
			}
		})
	}
}

func TestNetworkPurchaseQuoteGoldens(t *testing.T) {
	const project = "pro-00000000-0000-0000-0000-000000000001"
	const region = "66b500000000000000000001"
	const category = "66b500000000000000000009"
	const service = "00000000-0000-0000-0000-0000000000a1"
	const endpointBase = "/vnetwork-gateway/vnetwork/v1/" + region + "/" + project + "/endpoints"
	const natBase = "/vnetwork-gateway/vnetwork/v1/zone-1/proj-1/nats"
	for _, tc := range []struct {
		name, project, price, resource string
		args                           []string
		routes                         map[string]string
	}{
		{"quote-create-endpoint", project, "/vnetwork-gateway/vnetwork/billing/v1/" + region + "/" + project + "/price", "endpoint", []string{"--name", "edge", "--vpc-id", epCLIVPC, "--subnet-id", epCLISubnet}, map[string]string{
			"/vserver/iam-vserver-gateway/v1/projects": "endpoint_quote_projects.json",
			"/vnetwork-gateway/vnetwork/v1/regions":    "endpoint_quote_regions.json",
			endpointBase + "/zones":                    "endpoint_quote_zones.json", endpointBase + "/categories": "endpoint_quote_categories.json",
			endpointBase + "/" + category + "/services": "endpoint_quote_services.json", endpointBase + "/" + service + "/packages": "endpoint_quote_packages.json",
			endpointBase + "/vpcs": "endpoint_quote_vpcs.json", endpointBase + "/vpcs/" + epCLIVPC + "/subnets": "endpoint_quote_subnets.json",
		}},
		{"quote-create-nat-instance", "proj-1", "/vnetwork-gateway/vnetwork/billing/v1/zone-1/proj-1/price", "nat", []string{"--name", "edge", "--zone-id", "zone-1", "--availability-zone-id", "az-1", "--package-id", "package-1", "--vpc-id", "vpc-1"}, map[string]string{
			natBase + "/v3-whitelist": "nat_quote_whitelist.json", natBase + "/zones": "list_nat_zones.json", natBase + "/nat-package": "list_nat_packages.json", natBase + "/vpcs": "nat_quote_vpcs.json",
		}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.name+"/"+format, func(t *testing.T) {
				withCleanEnv(t)
				used := map[string]bool{}
				transport := saCLITransport(func(r *http.Request) (*http.Response, error) {
					w := httptest.NewRecorder()
					switch r.URL.Host {
					case "signin.greennode.ai":
						if r.Method == http.MethodGet {
							_, _ = io.WriteString(w, `<input name="_csrf" value="synthetic">`)
						} else {
							w.Header().Set("Location", "https://dashboard.console.greennode.ai/?code=synthetic")
							w.WriteHeader(http.StatusFound)
						}
						return w.Result(), nil
					case "dashboard.console.greennode.ai":
						_, _ = io.WriteString(w, `{"accessToken":"toy-token","expiresIn":3600}`)
						return w.Result(), nil
					}
					path := r.URL.Path
					method := http.MethodGet
					fixture := tc.routes[path]
					host := "hcm-3-vnetwork.console.greennode.ai"
					if fixture == "endpoint_quote_projects.json" {
						host = "hcm-3.console.greennode.ai"
					}
					if path == tc.price {
						method = http.MethodPost
						fixture = "quote_create_nat.json"
						var req struct {
							ResourceType string `json:"resourceType"`
							Action       string `json:"action"`
						}
						if json.NewDecoder(r.Body).Decode(&req) != nil || req.ResourceType != tc.resource || req.Action != "create" {
							t.Fatal("unexpected quote body")
						}
					}
					if fixture == "" || r.Method != method || r.URL.Host != host || used[path] {
						t.Fatalf("unexpected request for %s", path)
					}
					used[path] = true
					body, err := os.ReadFile(filepath.Join(os.Getenv("NETWORK_FIXTURE_DIR"), fixture))
					if err != nil {
						t.Fatal(err)
					}
					_, _ = w.Write(body)
					return w.Result(), nil
				})
				if tc.resource == "endpoint" {
					withTestOptions(t, vngcloud.WithIAMUser(&vngcloud.IAMUserAuth{RootEmail: "root", Username: "user", Password: "synthetic-password", HTTPClient: &http.Client{Transport: transport}}), vngcloud.WithTransport(transport), vngcloud.WithRetry(0, 0))
				} else {
					withTestOptions(t, vngcloud.WithStaticToken("toy-token"), vngcloud.WithTransport(transport), vngcloud.WithRetry(0, 0))
				}
				var stdout, stderr strings.Builder
				root := newRootCmd(strings.NewReader(""), &stdout, &stderr)
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", tc.project, "--read-only", "--output", format, "network", tc.name}, tc.args...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if len(used) != len(tc.routes)+1 || stderr.Len() != 0 {
					t.Fatal("unexpected call count or stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("NETWORK_GOLDEN_DIR"), tc.name+"."+format+".golden"), []byte(stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
