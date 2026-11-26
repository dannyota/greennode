package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadBalancerPortPolicyWrites(t *testing.T) {
	const root = "/v2/proj-1/loadBalancers/lb-1"
	const policies = root + "/listeners/listener-1/l7policies"
	for _, c := range []struct {
		name string
		args []string
	}{
		{"create-policy", []string{"--name", "routing", "--action", "REDIRECT_TO_POOL", "--redirect-pool-id", "pool-1", "--cli-input-json", `{"Rules":[{"Type":"PATH","CompareType":"STARTS_WITH","Value":"/api"}]}`}},
		{"update-policy", []string{"--policy-id", "policy-1", "--action", "REDIRECT_TO_URL", "--redirect-url", "https://example.test/api", "--redirect-http-code", "302", "--keep-query-string=false"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					root: func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}`))
					},
					policies: func(w http.ResponseWriter, r *http.Request) {
						if r.Method != http.MethodPost {
							t.Error("expected POST")
							return
						}
						w.WriteHeader(http.StatusAccepted)
						_, _ = w.Write([]byte(`{"uuid":"policy-1"}`))
					},
					policies + "/policy-1": func(w http.ResponseWriter, r *http.Request) {
						if r.Method == http.MethodPut {
							w.WriteHeader(http.StatusAccepted)
							return
						}
						_, _ = w.Write([]byte(`{"data":{"uuid":"policy-1","name":"routing","action":"REDIRECT_TO_POOL","redirectPoolId":"pool-1","progressStatus":"CREATED","l7Rules":[{"uuid":"rule-1","compareType":"STARTS_WITH","ruleValue":"/api","ruleType":"PATH"}]}}`))
					},
				})
				cmd, stdout, stderr := newSvcRoot(t, fixture)
				cmd.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "loadbalancer", c.name, "--load-balancer-id", "lb-1", "--listener-id", "listener-1", "--no-wait"}, c.args...))
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
