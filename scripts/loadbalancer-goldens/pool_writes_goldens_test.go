package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadBalancerPortPoolWrites(t *testing.T) {
	const lb = "/v2/proj-1/loadBalancers/lb-1"
	const pool = lb + "/pools/pool-1"
	for _, c := range []struct {
		name string
		args []string
	}{
		{"create-pool", []string{"--name", "web", "--protocol", "HTTP", "--health-check-protocol", "HTTP"}},
		{"update-pool", []string{"--pool-id", "pool-1", "--algorithm", "LEAST_CONNECTIONS", "--stickiness=false"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					lb: func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}`))
					},
					lb + "/pools": func(w http.ResponseWriter, r *http.Request) {
						if r.Method != http.MethodPost {
							t.Fatal("expected POST")
						}
						w.WriteHeader(http.StatusAccepted)
						_, _ = w.Write([]byte(`{"uuid":"pool-1"}`))
					},
					pool: func(w http.ResponseWriter, r *http.Request) {
						if r.Method == http.MethodPut {
							w.WriteHeader(http.StatusAccepted)
							return
						}
						_, _ = w.Write([]byte(`{"data":{"uuid":"pool-1","name":"web","protocol":"HTTP","loadBalanceMethod":"ROUND_ROBIN","progressStatus":"CREATED"}}`))
					},
					pool + "/healthMonitor": func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"healthCheckProtocol":"HTTP","healthyThreshold":4,"unhealthyThreshold":5,"interval":30,"timeout":5,"domainName":"example.test"}}`))
					},
				})
				root, stdout, stderr := newSvcRoot(t, fixture)
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "loadbalancer", c.name, "--load-balancer-id", "lb-1", "--no-wait"}, c.args...))
				if err := root.Execute(); err != nil {
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
