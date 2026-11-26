package cli

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
)

func TestLoadBalancerPortChildDeletePartial(t *testing.T) {
	const lb = "/v2/proj-1/loadBalancers/lb-1"
	var deleted atomic.Bool
	fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
		lb: func(w http.ResponseWriter, r *http.Request) {
			_, _ = w.Write([]byte(`{"data":{"uuid":"lb-1","progressStatus":"CREATED"}}`))
		},
		lb + "/listeners/listener-1": func(w http.ResponseWriter, r *http.Request) {
			if r.Method == http.MethodDelete {
				deleted.Store(true)
				w.WriteHeader(http.StatusAccepted)
			} else if deleted.Load() {
				w.WriteHeader(http.StatusForbidden)
				_, _ = w.Write([]byte(`{"message":"lookup denied"}`))
			} else {
				_, _ = w.Write([]byte(`{"data":{"uuid":"listener-1","progressStatus":"CREATED"}}`))
			}
		},
	})
	root, stdout, stderr := newSvcRoot(t, fixture)
	root.SetArgs([]string{"--region", "hcm-3", "--project-id", "proj-1", "loadbalancer", "delete-listener", "--load-balancer-id", "lb-1", "--listener-id", "listener-1", "--yes"})
	err := root.Execute()
	if err == nil || !deleted.Load() {
		t.Fatal("expected a confirmation error after delete")
	}
	printError(stderr, err)
	for name, data := range map[string][]byte{"delete-listener.partial.json.golden": stdout.Bytes(), "delete-listener.partial.stderr.golden": stderr.Bytes()} {
		if err := os.WriteFile(filepath.Join(os.Getenv("LOADBALANCER_GOLDEN_DIR"), name), data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
}

func TestLoadBalancerPortChildDeletes(t *testing.T) {
	const lb = "/v2/proj-1/loadBalancers/lb-1"
	for _, c := range []struct {
		name, path, id string
		args           []string
	}{
		{"delete-listener", "/listeners/listener-1", "listener-1", []string{"--listener-id", "listener-1"}},
		{"delete-policy", "/listeners/listener-1/l7policies/policy-1", "policy-1", []string{"--listener-id", "listener-1", "--policy-id", "policy-1"}},
		{"delete-pool", "/pools/pool-1", "pool-1", []string{"--pool-id", "pool-1"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				var deleted atomic.Bool
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					lb: func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"data":{"uuid":"lb-1","progressStatus":"CREATED"}}`))
					},
					lb + "/listeners": func(w http.ResponseWriter, r *http.Request) {
						_, _ = w.Write([]byte(`{"data":[]}`))
					},
					lb + c.path: func(w http.ResponseWriter, r *http.Request) {
						switch r.Method {
						case http.MethodGet:
							if deleted.Load() {
								w.WriteHeader(http.StatusNotFound)
								return
							}
							_, _ = fmt.Fprintf(w, `{"data":{"uuid":%q,"progressStatus":"CREATED"}}`, c.id)
						case http.MethodDelete:
							deleted.Store(true)
							w.WriteHeader(http.StatusAccepted)
						default:
							t.Error("unexpected method")
						}
					},
				})
				root, stdout, stderr := newSvcRoot(t, fixture)
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "loadbalancer", c.name, "--load-balancer-id", "lb-1", "--yes"}, c.args...))
				if err := root.Execute(); err != nil {
					t.Fatal(err)
				}
				if stderr.Len() != 0 || !deleted.Load() {
					t.Fatal("expected a successful delete")
				}
				name := fmt.Sprintf("%s.%s.golden", c.name, format)
				if err := os.WriteFile(filepath.Join(os.Getenv("LOADBALANCER_GOLDEN_DIR"), name), stdout.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
