package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
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

func TestComputeServerPortGoldens(t *testing.T) {
	priced := []string{"--zone-id", "zone-1", "--flavor-id", "flavor-1", "--image-id", "image-1", "--root-disk-size", "20", "--root-disk-type-id", "disk-1"}
	create := append(append([]string{}, priced...), "--name", "example", "--vpc-id", "vpc-1", "--subnet-id", "subnet-1", "--security-group-id", "group-1", "--ssh-key-id", "key-1", "--max-price", "20", "--no-wait")
	for _, c := range []struct {
		name  string
		flags []string
		quote string
	}{
		{"quote-create-server", priced, "server-order-quote"},
		{"quote-resize-server", []string{"--server-id", "server-1", "--flavor-id", "flavor-2"}, "server-resize-quote"},
		{"create-server", create, "server-order-quote"},
		{"delete-server", []string{"--server-id", "server-1", "--no-wait", "--yes"}, ""},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				reply := func(w http.ResponseWriter, status int, name string) {
					var body []byte
					if name != "" {
						var err error
						body, err = os.ReadFile(filepath.Join(os.Getenv("COMPUTE_WIRE_DIR"), name+".json"))
						if err != nil {
							t.Fatal(err)
						}
					}
					w.WriteHeader(status)
					_, _ = w.Write(body)
				}
				routes := map[string]func(http.ResponseWriter, *http.Request){
					"/v1/price": func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, c.quote) },
					"/v2/proj-1/servers": func(w http.ResponseWriter, r *http.Request) {
						if r.Method == "POST" {
							reply(w, 202, "server-order-accepted")
							return
						}
						reply(w, 200, "server-order-inventory")
					},
					"/v2/proj-1/servers/server-1": func(w http.ResponseWriter, r *http.Request) {
						if r.Method == "DELETE" {
							reply(w, 202, "")
							return
						}
						reply(w, 200, "server-delete-inventory")
					},
					"/v2/proj-1/volumes/servers/server-1": func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, "server-delete-volumes") },
				}
				root, stdout, stderr := newSvcRoot(t, newSvcFixture(routes))
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "compute", c.name}, c.flags...))
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

func TestComputeActionPortGoldens(t *testing.T) {
	for _, c := range []struct {
		name, current string
		flags         []string
	}{
		{"start-server", "server-action-stopped", []string{"--no-wait"}},
		{"stop-server", "server-action-active", []string{"--no-wait", "--yes"}},
		{"reboot-server", "server-action-active", []string{"--no-wait", "--yes"}},
		{"rename-server", "server-action-renamed", []string{"--name", "renamed"}},
		{"resize-server", "server-action-active", []string{"--flavor-id", "flavor-2", "--no-wait", "--yes"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				reply := func(w http.ResponseWriter, status int, name string) {
					var body []byte
					if name != "" {
						var err error
						body, err = os.ReadFile(filepath.Join(os.Getenv("COMPUTE_WIRE_DIR"), name+".json"))
						if err != nil {
							t.Fatal(err)
						}
					}
					w.WriteHeader(status)
					_, _ = w.Write(body)
				}
				routes := map[string]func(http.ResponseWriter, *http.Request){
					"/v1/price":                   func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, "server-resize-quote") },
					"/v2/proj-1/servers/server-1": func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, c.current) },
					"/v2/proj-1/servers/server-1/" + strings.TrimSuffix(c.name, "-server"): func(w http.ResponseWriter, _ *http.Request) {
						if c.name == "rename-server" {
							reply(w, 200, c.current)
							return
						}
						reply(w, 202, "")
					},
				}
				root, stdout, stderr := newSvcRoot(t, newSvcFixture(routes))
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "compute", c.name, "--server-id", "server-1"}, c.flags...))
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

func TestComputeGroupPortGoldens(t *testing.T) {
	for _, c := range []struct {
		name  string
		flags []string
	}{
		{"create-server-group", []string{"--name", "example", "--policy-id", "policy-1", "--description", "toy"}},
		{"update-server-group", []string{"--server-group-id", "group-1", "--description", "toy"}},
		{"delete-server-group", []string{"--server-group-id", "group-1", "--yes"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				reply := func(w http.ResponseWriter, status int, name string) {
					var body []byte
					if name != "" {
						var err error
						body, err = os.ReadFile(filepath.Join(os.Getenv("COMPUTE_WIRE_DIR"), name+".json"))
						if err != nil {
							t.Fatal(err)
						}
					}
					w.WriteHeader(status)
					_, _ = w.Write(body)
				}
				reads := 0
				routes := map[string]func(http.ResponseWriter, *http.Request){
					"/v2/proj-1/serverGroups": func(w http.ResponseWriter, r *http.Request) {
						if r.Method == "POST" {
							reply(w, 201, "group-result")
							return
						}
						reply(w, 200, "group-empty-inventory")
					},
					"/v2/proj-1/serverGroups/group-1": func(w http.ResponseWriter, r *http.Request) {
						switch r.Method {
						case "DELETE":
							reply(w, 204, "")
						case "PUT":
							reply(w, 200, "")
						default:
							reads++
							if reads == 1 {
								reply(w, 200, "group-current")
								return
							}
							reply(w, 200, "group-result")
						}
					},
				}
				root, stdout, stderr := newSvcRoot(t, newSvcFixture(routes))
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "compute", c.name}, c.flags...))
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

func TestComputeSSHPortGoldens(t *testing.T) {
	for _, c := range []struct {
		name, route, wire string
		status            int
		flags             []string
	}{
		{"create-ssh-key", "/v2/proj-1/sshKeys", "ssh-key-created", 201, []string{"--name", "example"}},
		{"import-ssh-key", "/v2/proj-1/sshKeys/import", "ssh-key-imported", 201, []string{"--name", "example", "--public-key", "ssh-rsa toy public"}},
		{"delete-ssh-key", "/v2/proj-1/sshKeys/key-1", "", 204, []string{"--ssh-key-id", "key-1", "--yes"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				var body []byte
				if c.wire != "" {
					var err error
					body, err = os.ReadFile(filepath.Join(os.Getenv("COMPUTE_WIRE_DIR"), c.wire+".json"))
					if err != nil {
						t.Fatal(err)
					}
				}
				routes := map[string]func(http.ResponseWriter, *http.Request){c.route: func(w http.ResponseWriter, _ *http.Request) {
					w.WriteHeader(c.status)
					_, _ = w.Write(body)
				}}
				root, stdout, stderr := newSvcRoot(t, newSvcFixture(routes))
				args := append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "compute", c.name}, c.flags...)
				if c.name == "create-ssh-key" {
					args = append(args, "--secret-file", filepath.Join(t.TempDir(), "toy.key"), "--query", "{SSHKey: SSHKey, PrivateKey: PrivateKey, SecretFile: contains(SecretFile, 'toy.key')}")
				}
				root.SetArgs(args)
				if err := root.ExecuteContext(computeDiscoveryCLIContext()); err != nil {
					t.Fatal(err)
				}
				if stderr.Len() != 0 || strings.Contains(stdout.String(), "toy-private-key") {
					t.Fatal("unexpected output")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("COMPUTE_GOLDEN_DIR"), fmt.Sprintf("%s.%s.golden", c.name, format)), stdout.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestComputeFloatingPortGoldens(t *testing.T) {
	for _, format := range []string{"json", "table", "text"} {
		t.Run(format, func(t *testing.T) {
			reply := func(w http.ResponseWriter, status int, name string) {
				body, err := os.ReadFile(filepath.Join(os.Getenv("COMPUTE_WIRE_DIR"), name+".json"))
				if err != nil {
					t.Fatal(err)
				}
				w.WriteHeader(status)
				_, _ = w.Write(body)
			}
			routes := map[string]func(http.ResponseWriter, *http.Request){
				"/v2/proj-1/servers": func(w http.ResponseWriter, r *http.Request) {
					if r.Method == "POST" {
						reply(w, 202, "server-order-accepted")
						return
					}
					reply(w, 200, "server-order-inventory")
				},
				"/v2/proj-1/secgroups/sg-1":               func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, "server-floating-group") },
				"/v2/proj-1/secgroups/sg-1/secGroupRules": func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, "server-floating-rules") },
				"/v1/price":                   func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, "server-order-quote") },
				"/v2/proj-1/servers/server-1": func(w http.ResponseWriter, _ *http.Request) { reply(w, 200, "server-floating-active") },
			}
			root, stdout, stderr := newSvcRoot(t, newSvcFixture(routes))
			root.SetArgs([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "compute", "create-server",
				"--name", "web-1", "--zone-id", "zone-1", "--flavor-id", "flavor-1", "--image-id", "image-1", "--vpc-id", "vpc-1", "--subnet-id", "subnet-1", "--security-group-id", "sg-1", "--ssh-key-id", "key-1", "--root-disk-size", "20", "--root-disk-type-id", "disk-1", "--max-price", "20", "--attach-floating-ip"})
			if err := root.ExecuteContext(computeDiscoveryCLIContext()); err != nil {
				t.Fatal(err)
			}
			if stderr.Len() != 0 {
				t.Fatal("unexpected stderr")
			}
			if err := os.WriteFile(filepath.Join(os.Getenv("COMPUTE_GOLDEN_DIR"), fmt.Sprintf("create-server-floating-ip.%s.golden", format)), stdout.Bytes(), 0o644); err != nil {
				t.Fatal(err)
			}
		})
	}
}
