package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestComputePortGoldens(t *testing.T) {
	cases := []struct {
		name, route string
		flags       []string
	}{
		{"get-server", "/v2/proj-1/servers/server-1", []string{"--server-id", "server-1"}},
		{"get-server-console-log", "/v2/proj-1/servers/server-1/console-log", []string{"--server-id", "server-1"}},
		{"get-server-group", "/v2/proj-1/serverGroups/group-1", []string{"--server-group-id", "group-1"}},
		{"get-ssh-key", "/v2/proj-1/sshKeys/key-1", []string{"--ssh-key-id", "key-1"}},
		{"list-flavor-zones", "/v1/proj-1/flavor_zones/product", []string{"--zone-id", "zone-1"}},
		{"list-flavors", "/v1/proj-1/general-1/flavors", []string{"--flavor-zone-id", "general-1"}},
		{"list-gpu-images", "/v1/proj-1/images/gpu", []string{}},
		{"list-os-images", "/v1/proj-1/images/os", []string{"--zone-id", "zone-1"}},
		{"list-server-group-members", "/v2/proj-1/serverGroups", []string{}},
		{"list-server-group-policies", "/v2/proj-1/serverGroups/policies", []string{}},
		{"list-server-groups", "/v2/proj-1/serverGroups", []string{}},
		{"list-server-security-groups", "/v2/proj-1/servers", []string{}},
		{"list-servers", "/v2/proj-1/servers", []string{}},
		{"list-ssh-keys", "/v2/proj-1/sshKeys", []string{}},
		{"list-user-images", "/v2/proj-1/user-images", []string{}},
	}
	for _, c := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				body, err := os.ReadFile(filepath.Join(os.Getenv("COMPUTE_WIRE_DIR"), c.name+".json"))
				if err != nil {
					t.Fatal(err)
				}
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){c.route: func(w http.ResponseWriter, r *http.Request) {
					if r.Method != http.MethodGet {
						t.Error("unexpected method")
					}
					_, _ = w.Write(body)
				}})
				root, stdout, stderr := newSvcRoot(t, fixture)
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--read-only", "--output", format, "compute", c.name}, c.flags...))
				if err := root.ExecuteContext(computeDiscoveryCLIContext()); err != nil {
					t.Fatal(err)
				}
				if stderr.Len() != 0 {
					t.Fatal("unexpected stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("COMPUTE_GOLDEN_DIR"), fmt.Sprintf("%s.%s.golden", c.name, format)), stdout.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestComputePricePortGoldens(t *testing.T) {
	routes := map[string]func(http.ResponseWriter, *http.Request){}
	for route, name := range map[string]string{
		"/v1/proj-1/zones":                                  "zones",
		"/v1/proj-1/flavor_zones/codes":                     "platforms",
		"/v1/proj-1/flavor_zones/families":                  "families",
		"/v1/proj-1/flavors/families/general/platforms/cpu": "flavors",
		"/v1/proj-1/volume_type_zones":                      "disk-groups",
		"/v1/proj-1/pool-1/volume_types":                    "disks",
		"/v1/proj-1/images/os":                              "images",
		"/v1/price":                                         "quote",
	} {
		body, err := os.ReadFile(filepath.Join(os.Getenv("COMPUTE_WIRE_DIR"), "price-"+name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		routes[route] = func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write(body) }
	}
	for _, format := range []string{"json", "table", "text"} {
		t.Run(format, func(t *testing.T) {
			root, stdout, stderr := newSvcRoot(t, newSvcFixture(routes))
			args := []string{"--region", "hcm-3", "--project-id", "proj-1", "--read-only", "--output", format, "compute", "list-flavor-prices", "--root-disk-type-id", "disk-1", "--zone-id", "zone-1"}
			if format == "json" {
				args = append(args, "--query", "{Items:Items,Requests:Metadata.Requests}")
			}
			root.SetArgs(args)
			if err := root.ExecuteContext(computeDiscoveryCLIContext()); err != nil {
				t.Fatal(err)
			}
			for suffix, body := range map[string][]byte{"": stdout.Bytes(), ".stderr": stderr.Bytes()} {
				if err := os.WriteFile(filepath.Join(os.Getenv("COMPUTE_GOLDEN_DIR"), "list-flavor-prices."+format+suffix+".golden"), body, 0o644); err != nil {
					t.Fatal(err)
				}
			}
		})
	}
}
