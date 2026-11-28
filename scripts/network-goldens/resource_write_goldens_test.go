package cli

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/spf13/cobra"

	"danny.vn/vngcloud/network"
)

func TestNetworkResourceWriteGoldens(t *testing.T) {
	group := network.SecurityGroup{ID: "secg-1", Name: "<name>", Description: "<description>", Status: "ACTIVE", CreatedAt: "2026-01-01T00:00:00Z"}
	rule := network.SecurityGroupRule{ID: "secr-1", SecurityGroupID: "secg-1", Direction: "ingress", EtherType: "IPv4", Protocol: "tcp", PortRangeMin: 22, PortRangeMax: 22, RemoteIPPrefix: "203.0.113.0/24", Description: "example"}
	vip := network.VirtualIPAddress{UUID: "vip-1", Name: "<name>", EndpointAddress: "<ip>", VPCID: "vpc-1", SubnetID: "subnet-1", Description: "<description>", Mode: "Active/Passive", Type: "private", Status: "ACTIVE"}
	updatedVIP := vip
	updatedVIP.Mode = "Active/Active"
	var nat struct{ Data []network.NATInstance }
	wire, err := os.ReadFile(filepath.Join("..", "..", "testdata", "network", "nat_write_inventory.json"))
	if err != nil || json.Unmarshal(wire, &nat) != nil || len(nat.Data) != 1 {
		t.Fatal("invalid NAT fixture")
	}
	renewal := false
	for _, tc := range []struct {
		name  string
		input string
		out   any
	}{
		{"create-security-group", `{"Name":"example"}`, &network.CreateSecurityGroupOutput{SecurityGroup: group}},
		{"update-security-group", `{"SecurityGroupID":"secg-1","Name":"example"}`, &network.UpdateSecurityGroupOutput{SecurityGroup: group}},
		{"delete-security-group", `{"SecurityGroupID":"secg-1"}`, &network.DeleteSecurityGroupOutput{}},
		{"create-security-group-rule", `{"SecurityGroupID":"secg-1","Direction":"ingress","Protocol":"tcp","RemoteIPPrefix":"203.0.113.0/24","PortRangeMin":22,"Description":"example"}`, &network.CreateSecurityGroupRuleOutput{SecurityGroupRule: rule}},
		{"delete-security-group-rule", `{"SecurityGroupID":"secg-1","SecurityGroupRuleID":"secr-1"}`, &network.DeleteSecurityGroupRuleOutput{}},
		{"create-virtual-ip-address", `{"SubnetID":"subnet-1","Name":"example","Mode":"Active/Passive"}`, &network.CreateVirtualIPAddressOutput{VirtualIPAddress: vip}},
		{"update-virtual-ip-address", `{"VirtualIPAddressID":"vip-1","Mode":"Active/Active"}`, &network.UpdateVirtualIPAddressOutput{VirtualIPAddress: updatedVIP}},
		{"delete-virtual-ip-address", `{"VirtualIPAddressID":"vip-1"}`, &network.DeleteVirtualIPAddressOutput{}},
		{"create-endpoint", `{"Name":"demo","VPCID":"` + epCLIVPC + `","SubnetID":"` + epCLISubnet + `","MaxPrice":712400}`, &network.CreateEndpointOutput{EndpointID: epCLIID, Status: "ACTIVE", MonthlyPrice: 712400, Currency: "VND"}},
		{"delete-endpoint", `{"EndpointID":"` + epCLIID + `","VPCID":"` + epCLIVPC + `"}`, &network.DeleteEndpointOutput{EndpointID: epCLIID, Deleted: true}},
		{"create-nat-instance", `{"Name":"example","ZoneID":"66b500000000000000000001","AvailabilityZoneID":"HAN01-1B","PackageID":"package-1","VPCID":"vpc-1","MaxPrice":712400}`, &network.CreateNATInstanceOutput{NATInstance: &nat.Data[0], MonthlyPrice: 712400, TotalPrice: 712400, Currency: "VND", AutoRenew: &renewal}},
		{"delete-nat-instance", `{"ZoneID":"66b500000000000000000001","VPCID":"vpc-1","NATID":"nat-1"}`, &network.DeleteNATInstanceOutput{}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.name+"/"+format, func(t *testing.T) {
				h := newFakeHarness(t)
				op := natWriteOp(t, tc.name)
				op.call = func(_ *cobra.Command, _ *network.Client, _ context.Context, _ any) (any, error) {
					return tc.out, nil
				}
				root := newTestRoot(h.e)
				root.AddCommand(Service(h.e, "network", "test", network.New, op))
				if err := execCmd(t, root, []string{"--output", format, "network", tc.name, "--yes", "--cli-input-json", tc.input}); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("NETWORK_GOLDEN_DIR"), tc.name+"."+format+".golden"), []byte(h.stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
