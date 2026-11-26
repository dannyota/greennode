package cli

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"

	"danny.vn/vngcloud/globalloadbalancer"
)

func TestGlobalLoadBalancerWriteGoldens(t *testing.T) {
	for _, test := range []struct {
		command string
		result  any
		partial any
	}{
		{"create-load-balancer", &globalloadbalancer.CreateLoadBalancerOutput{LoadBalancer: globalloadbalancer.LoadBalancer{ID: "glb-1", Name: "demo", Package: "pkg-1", Type: "Layer 4", Status: "ACTIVE"}}, &globalloadbalancer.CreateLoadBalancerOutput{LoadBalancer: globalloadbalancer.LoadBalancer{ID: "glb-1"}}},
		{"delete-load-balancer", &globalloadbalancer.DeleteLoadBalancerOutput{LoadBalancerID: "glb-1", Deleted: true}, &globalloadbalancer.DeleteLoadBalancerOutput{LoadBalancerID: "glb-1"}},
	} {
		for _, format := range []string{"json", "table", "text", "partial.json"} {
			var output bytes.Buffer
			if format == "partial.json" {
				if err := renderOutput(&output, "json", "", test.partial, false); err != nil {
					t.Fatal(err)
				}
			} else if err := renderOutput(&output, format, "", test.result, true); err != nil {
				t.Fatal(err)
			}
			name := test.command + "." + format + ".golden"
			if err := os.WriteFile(filepath.Join(os.Getenv("GLB_GOLDEN_DIR"), name), output.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		}
	}
}
