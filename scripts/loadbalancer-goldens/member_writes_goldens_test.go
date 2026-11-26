package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadBalancerPortMemberWrites(t *testing.T) {
	const root = "/v2/proj-1/loadBalancers/lb-1"
	const pool = root + "/pools/pool-1"
	for _, c := range []struct {
		name string
		args []string
	}{
		{"add-pool-member", []string{"--address", "192.0.2.11", "--name", "secondary"}},
		{"update-pool-member", []string{"--address", "192.0.2.10", "--backup=false", "--weight", "2"}},
		{"remove-pool-member", []string{"--address", "192.0.2.10", "--yes"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					root: func(w http.ResponseWriter, _ *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"uuid":"lb-1","type":"Layer 4","progressStatus":"CREATED"}}`))
					},
					pool: func(w http.ResponseWriter, _ *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"uuid":"pool-1","name":"backends","progressStatus":"CREATED","members":[{"address":"192.0.2.10","protocolPort":80,"weight":1}]}}`))
					},
					pool + "/members": func(w http.ResponseWriter, r *http.Request) {
						if r.Method == http.MethodPut {
							w.WriteHeader(http.StatusAccepted)
							return
						}
						_, _ = w.Write([]byte(`{"data":[{"uuid":"member-1","address":"192.0.2.10","protocolPort":80,"name":"primary","weight":1,"backup":true}]}`))
					},
				})
				cmd, stdout, stderr := newSvcRoot(t, fixture)
				cmd.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "loadbalancer", c.name, "--load-balancer-id", "lb-1", "--pool-id", "pool-1", "--port", "80", "--no-wait"}, c.args...))
				if err := cmd.Execute(); err != nil {
					t.Fatal(err)
				}
				if stderr.Len() != 0 {
					t.Fatal("expected a successful write")
				}
				name := fmt.Sprintf("%s.%s.golden", c.name, format)
				if err := os.WriteFile(filepath.Join(os.Getenv("LOADBALANCER_GOLDEN_DIR"), name), stdout.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
