package cli

import (
	"context"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"danny.vn/vngcloud/vks"
	"github.com/spf13/cobra"
)

func TestVKSCreateGoldens(t *testing.T) {
	for _, tc := range []struct {
		name, status string
		err          error
	}{
		{"create-cluster", "ACTIVE", nil},
		{"create-cluster-partial", "CREATING", vks.ErrNotSettled},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.name+"/"+format, func(t *testing.T) {
				h := newFakeHarness(t)
				op := Write[vks.Client, vks.CreateClusterInput, vks.CreateClusterOutput]("create-cluster", (*vks.Client).CreateCluster, Guard(requireVKSCreateConsent))
				op.call = func(_ *cobra.Command, _ *vks.Client, _ context.Context, _ any) (any, error) {
					return &vks.CreateClusterOutput{ID: "cluster-1", Status: tc.status}, tc.err
				}
				root := newTestRoot(h.e)
				root.AddCommand(Service(h.e, "vks", "test", vks.New, op))
				root.SetArgs([]string{"--region", "hcm-3", "--yes", "--output", format, "vks", "create-cluster", "--cli-input-json", `{"Name":"cluster","Version":"v1","VPCID":"vpc-1","SubnetID":"subnet-1","PodCIDR":"10.20.0.0/16","NodeGroupName":"group","NumNodes":1,"FlavorID":"flavor-1","OS":"ubuntu","DiskSize":20,"DiskTypeID":"disk-1","SecurityGroupIDs":["sg-1"],"SSHKeyID":"key-1"}`})
				if err := root.ExecuteContext(context.Background()); !errors.Is(err, tc.err) {
					t.Fatal(err)
				}
				path := filepath.Join(os.Getenv("VKS_GOLDEN_DIR"), tc.name+"."+format+".golden")
				if err := os.WriteFile(path, []byte(h.stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

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
