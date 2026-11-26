package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadBalancerPortListenerWrites(t *testing.T) {
	const root = "/v2/proj-1/loadBalancers/lb-1"
	const listener = root + "/listeners/listener-1"
	for _, c := range []struct {
		name string
		args []string
	}{
		{"create-listener", []string{
			"--name", "web", "--protocol", "HTTPS", "--port", "443", "--default-certificate-id", "cert-1",
			"--allowed-cidrs", "10.0.0.0/24, 100.64.0.0/10", "--cli-input-json",
			`{"AllowedCIDRs":["0.0.0.0/0"],"CertificateIDs":["cert-1"],"InsertHeaders":[{"HeaderName":"X-To","HeaderValue":"web"}]}`,
		}},
		{"update-listener", []string{"--listener-id", "listener-1", "--timeout-client", "0", "--client-certificate-id", ""}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					root: func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}`))
					},
					root + "/listeners": func(w http.ResponseWriter, r *http.Request) {
						if r.Method != http.MethodPost {
							t.Error("expected POST")
							return
						}
						w.WriteHeader(http.StatusAccepted)
						_, _ = w.Write([]byte(`{"uuid":"listener-1"}`))
					},
					listener: func(w http.ResponseWriter, r *http.Request) {
						if r.Method == http.MethodPut {
							w.WriteHeader(http.StatusOK)
							return
						}
						_, _ = w.Write([]byte(`{"data":{"uuid":"listener-1","name":"web","protocol":"HTTPS","protocolPort":443,"defaultPoolId":"pool-1","timeoutClient":51,"timeoutMember":52,"timeoutConnection":6,"allowedCidrs":"10.0.0.0/24","certificateAuthorities":["cert-1"],"defaultCertificateAuthority":"cert-1","clientCertificateAuthentication":"client-1","progressStatus":"CREATED","insertHeaders":[{"headerName":"X-To","headerValue":"web"}]}}`))
					},
				})
				cmd, stdout, stderr := newSvcRoot(t, fixture)
				cmd.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "loadbalancer", c.name, "--load-balancer-id", "lb-1", "--no-wait"}, c.args...))
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
