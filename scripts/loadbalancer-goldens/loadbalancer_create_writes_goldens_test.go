package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadBalancerPortCreateWrites(t *testing.T) {
	for _, tc := range []struct {
		name string
		args []string
	}{
		{"import-certificate", []string{"--name", "toy-ca", "--type", "CA", "--cli-input-json", `{"Certificate":"-----BEGIN CERTIFICATE-----\nY2E=\n-----END CERTIFICATE-----\n"}`}},
		{"create-load-balancer", []string{"--name", "toy-lb", "--type", "Layer 4", "--scheme", "Internal", "--subnet-id", "subnet-1", "--zone-id", "zone-1", "--package-id", "package-1", "--max-price", "9", "--no-wait"}},
		{"resize-load-balancer", []string{"--load-balancer-id", "lb-1", "--package-id", "package-2", "--no-wait"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.name+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					"/v2/proj-1/cas": func(w http.ResponseWriter, r *http.Request) {
						w.WriteHeader(http.StatusCreated)
						_, _ = w.Write([]byte(`{"data":{"uuid":"cert-1","name":"toy-ca","certificateType":"CA"}}`))
					},
					"/v1/price": func(w http.ResponseWriter, r *http.Request) {
						price := `{"optimumPrice":9}`
						if tc.name == "resize-load-balancer" {
							price = `{"optimumPrice":-3}`
						}
						_, _ = w.Write([]byte(price))
					},
					"/v2/proj-1/loadBalancers": func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"uuid":"lb-1"}`))
					},
					"/v2/proj-1/loadBalancers/lb-1": func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"uuid":"lb-1","packageId":"package-1","progressStatus":"CREATED"}}`))
					},
					"/v2/proj-1/loadBalancers/lb-1/resize": func(w http.ResponseWriter, r *http.Request) {
						w.WriteHeader(http.StatusAccepted)
					},
				})
				root, stdout, stderr := newSvcRoot(t, fixture)
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "loadbalancer", tc.name}, tc.args...))
				if err := root.Execute(); err != nil {
					t.Fatal(err)
				}
				if stderr.Len() != 0 {
					t.Fatal("unexpected stderr")
				}
				name := fmt.Sprintf("%s.%s.golden", tc.name, format)
				if err := os.WriteFile(filepath.Join(os.Getenv("LOADBALANCER_GOLDEN_DIR"), name), stdout.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
