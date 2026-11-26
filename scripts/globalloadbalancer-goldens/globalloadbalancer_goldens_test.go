package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestGlobalLoadBalancerReadGoldens(t *testing.T) {
	parent := "/global-load-balancers/glb-1"
	members := parent + "/global-pools/pool-1/pool-members"
	lb := []string{"--load-balancer-id", "glb-1"}
	pool := []string{"--load-balancer-id", "glb-1", "--pool-id", "pool-1"}
	for _, tc := range []struct {
		command, path string
		flags         []string
	}{
		{"list-packages", "/packages", nil},
		{"list-regions", "/regions", nil},
		{"list-load-balancers", "/global-load-balancers", []string{"--name", "app + blue", "--offset", "2", "--limit", "10"}},
		{"get-load-balancer", parent, lb},
		{"list-pools", parent + "/global-pools", lb},
		{"list-listeners", parent + "/global-listeners", lb},
		{"get-listener", parent + "/global-listeners/listener-1", []string{"--load-balancer-id", "glb-1", "--listener-id", "listener-1"}},
		{"list-pool-members", members, pool},
		{"get-pool-member", members + "/member-1", []string{"--load-balancer-id", "glb-1", "--pool-id", "pool-1", "--pool-member-id", "member-1"}},
		{"list-usage-histories", parent + "/usage-histories", []string{"--load-balancer-id", "glb-1", "--from", "toy + start", "--to", "toy-end", "--type", "traffic"}},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "glb", strings.ReplaceAll(tc.command, "-", "_")+".json"))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){"/v1" + tc.path: jsonHandler(http.StatusOK, string(body))})
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--read-only", "--output", format, "globalloadbalancer", tc.command}
				root.SetArgs(append(args, tc.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if fixture.requestCount() != 1 || stderr.Len() != 0 {
					t.Fatal("expected one request and empty stderr")
				}
				name := tc.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("GLB_GOLDEN_DIR"), name), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
