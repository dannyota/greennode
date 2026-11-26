package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"github.com/spf13/cobra"
	"danny.vn/vngcloud/vks"
)

func TestVKSDeleteGoldens(t *testing.T) {
	for _, format := range []string{"json", "table", "text"} {
		h := newFakeHarness(t)
		op := Write[vks.Client, vks.DeleteClusterInput, vks.DeleteClusterOutput]("delete-cluster", (*vks.Client).DeleteCluster, Destructive())
		op.call = func(_ *cobra.Command, _ *vks.Client, _ context.Context, _ any) (any, error) {
			return &vks.DeleteClusterOutput{Deleted: true}, nil
		}
		root := newTestRoot(h.e)
		root.AddCommand(Service(h.e, "vks", "test", vks.New, op))
		root.SetArgs([]string{"--region", "hcm-3", "--yes", "--output", format, "vks", "delete-cluster", "--cluster-id", "cluster-1"})
		if err := root.ExecuteContext(context.Background()); err != nil {
			t.Fatal(err)
		}
		path := filepath.Join(os.Getenv("VKS_GOLDEN_DIR"), "delete-cluster."+format+".golden")
		if err := os.WriteFile(path, []byte(h.stdout.String()), 0600); err != nil {
			t.Fatal(err)
		}
	}
}

func TestVKSGoldens(t *testing.T) {
	for _, tc := range []struct {
		command, route, fixture string
		flags                   []string
	}{
		{"list-clusters", "clusters", "cluster-list-schema.json", []string{"--page", "1", "--size", "2"}},
		{"list-cluster-versions", "cluster-versions", "hcm-cluster-versions.json", nil},
		{"get-quota", "quota", "hcm-quota.json", nil},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "vks", tc.fixture))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				path := "/vks-api/v1/" + tc.route
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					path: jsonHandler(http.StatusOK, string(body)),
				})
				root, stdout, stderr := newVKSRoot(t, fixture, false)
				args := []string{"--region", "hcm-3", "--read-only", "--output", format, "vks", tc.command}
				root.SetArgs(append(args, tc.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if fixture.requestCount() != 1 || stderr.Len() != 0 {
					t.Fatal("expected one request and empty stderr")
				}
				path = filepath.Join(os.Getenv("VKS_GOLDEN_DIR"), tc.command+"."+format+".golden")
				if err := os.WriteFile(path, stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
