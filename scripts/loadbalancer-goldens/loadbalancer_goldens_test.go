package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadBalancerPortErrors(t *testing.T) {
	const route = "/v2/proj-1/loadBalancers/lb-1/pools/pool-1/members"
	for _, c := range []struct {
		name   string
		status int
		body   string
	}{
		{"no-data", http.StatusOK, `{"data":null}`},
		{"bad-json", http.StatusOK, `{`},
		{"bad-shape", http.StatusOK, `{"data":{}}`},
		{"not-found", http.StatusNotFound, `{"message":"pool not found"}`},
		{"forbidden", http.StatusForbidden, `{"message":"access denied"}`},
	} {
		t.Run(c.name, func(t *testing.T) {
			fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){route: func(w http.ResponseWriter, _ *http.Request) {
				w.WriteHeader(c.status)
				_, _ = w.Write([]byte(c.body))
			}})
			root, stdout, stderr := newSvcRoot(t, fixture)
			root.SetArgs([]string{"--region", "hcm-3", "--project-id", "proj-1", "loadbalancer", "list-pool-members", "--load-balancer-id", "lb-1", "--pool-id", "pool-1"})
			err := root.Execute()
			if err == nil || stdout.Len() != 0 || stderr.Len() != 0 {
				t.Fatal("expected one command error")
			}
			printError(stderr, err)
			if err := os.WriteFile(filepath.Join(os.Getenv("LOADBALANCER_GOLDEN_DIR"), c.name+".stderr.golden"), stderr.Bytes(), 0o644); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestLoadBalancerPortGoldens(t *testing.T) {
	cases := []struct {
		name, route string
		flags       []string
	}{
		{"list-load-balancers", "/v2/proj-1/loadBalancers", []string{"--name", "blue team", "--page", "2", "--size", "5"}},
		{"get-load-balancer", "/v2/proj-1/loadBalancers/lb-1", []string{"--load-balancer-id", "lb-1"}},
		{"list-packages", "/v2/proj-1/loadBalancers/packages", []string{"--zone-id", "zone_1"}},
		{"list-certificates", "/v2/proj-1/cas", []string{}},
		{"get-certificate", "/v2/proj-1/cas/cert-1", []string{"--certificate-id", "cert-1"}},
		{"list-listeners", "/v2/proj-1/loadBalancers/lb-1/listeners", []string{"--load-balancer-id", "lb-1"}},
		{"get-listener", "/v2/proj-1/loadBalancers/lb-1/listeners/listener-1", []string{"--load-balancer-id", "lb-1", "--listener-id", "listener-1"}},
		{"list-pools", "/v2/proj-1/loadBalancers/lb-1/pools", []string{"--load-balancer-id", "lb-1"}},
		{"get-pool", "/v2/proj-1/loadBalancers/lb-1/pools/pool-1", []string{"--load-balancer-id", "lb-1", "--pool-id", "pool-1"}},
		{"get-pool-health-monitor", "/v2/proj-1/loadBalancers/lb-1/pools/pool-1/healthMonitor", []string{"--load-balancer-id", "lb-1", "--pool-id", "pool-1"}},
		{"list-pool-members", "/v2/proj-1/loadBalancers/lb-1/pools/pool-1/members", []string{"--load-balancer-id", "lb-1", "--pool-id", "pool-1"}},
		{"list-policies", "/v2/proj-1/loadBalancers/lb-1/listeners/listener-1/l7policies", []string{"--load-balancer-id", "lb-1", "--listener-id", "listener-1"}},
		{"get-policy", "/v2/proj-1/loadBalancers/lb-1/listeners/listener-1/l7policies/policy-1", []string{"--load-balancer-id", "lb-1", "--listener-id", "listener-1", "--policy-id", "policy-1"}},
		{"list-tags", "/v2/proj-1/tag/resource/lb-1", []string{"--load-balancer-id", "lb-1"}},
		{"quote-create-load-balancer", "/v1/price", []string{"--package-id", "package-1", "--zone-id", "zone_1", "--scheme", "Internal", "--subnet-id", "subnet-1"}},
		{"quote-resize-load-balancer", "/v1/price", []string{"--load-balancer-id", "lb-1", "--package-id", "package-1"}},
	}
	for _, c := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				body, err := os.ReadFile(filepath.Join(os.Getenv("LOADBALANCER_WIRE_DIR"), c.name+".json"))
				if err != nil {
					t.Fatal(err)
				}
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){c.route: func(w http.ResponseWriter, r *http.Request) {
					method := http.MethodGet
					if c.route == "/v1/price" {
						method = http.MethodPost
					}
					if r.Method != method {
						t.Error("unexpected method")
					}
					_, _ = w.Write(body)
				}})
				root, stdout, stderr := newSvcRoot(t, fixture)
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--read-only", "--output", format, "loadbalancer", c.name}, c.flags...))
				if err := root.Execute(); err != nil {
					t.Fatal(err)
				}
				if stderr.Len() != 0 {
					t.Fatal("unexpected stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("LOADBALANCER_GOLDEN_DIR"), fmt.Sprintf("%s.%s.golden", c.name, format)), stdout.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
