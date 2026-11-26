package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"danny.vn/vngcloud/globalloadbalancer"
)

func TestGlobalLoadBalancerChildWriteGoldens(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("GLB_CHILD_FIXTURE"))
	if err != nil {
		t.Fatal(err)
	}
	var wire struct {
		Pool     globalloadbalancer.Pool
		Group    globalloadbalancer.PoolMember
		Listener globalloadbalancer.Listener
	}
	if err := json.Unmarshal(raw, &wire); err != nil {
		t.Fatal(err)
	}
	for _, test := range []struct {
		command string
		result  any
		partial any
	}{
		{"create-pool", &globalloadbalancer.CreatePoolOutput{Pool: wire.Pool, PoolMember: wire.Group}, &globalloadbalancer.CreatePoolOutput{Pool: globalloadbalancer.Pool{ID: "pool-1"}, PoolMember: globalloadbalancer.PoolMember{ID: "group-1"}}},
		{"create-listener", &globalloadbalancer.CreateListenerOutput{Listener: wire.Listener}, &globalloadbalancer.CreateListenerOutput{Listener: globalloadbalancer.Listener{ID: "listener-1"}}},
		{"delete-pool", &globalloadbalancer.DeletePoolOutput{LoadBalancerID: "glb-1", PoolID: "pool-1", Deleted: true}, &globalloadbalancer.DeletePoolOutput{LoadBalancerID: "glb-1", PoolID: "pool-1"}},
		{"delete-listener", &globalloadbalancer.DeleteListenerOutput{LoadBalancerID: "glb-1", ListenerID: "listener-1", Deleted: true}, &globalloadbalancer.DeleteListenerOutput{LoadBalancerID: "glb-1", ListenerID: "listener-1"}},
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
			if err := os.WriteFile(filepath.Join(os.Getenv("GLB_GOLDEN_DIR"), test.command+"."+format+".golden"), output.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		}
	}
}
