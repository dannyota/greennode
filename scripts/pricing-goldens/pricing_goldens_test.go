package cli

import (
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"danny.vn/vngcloud/internal/testutil"
)

func TestPricingPortGoldens(t *testing.T) {
	dir := os.Getenv("PRICING_GOLDEN_DIR")
	if dir == "" {
		t.Fatal("PRICING_GOLDEN_DIR is required")
	}
	cases := []struct {
		name, file string
		code, exit int
		args       []string
	}{
		{"get-quote", "pricing/GetQuoteServerResizeCredit.json", 200, 0, []string{"get-quote", "--resource-type", "server", "--action", "resize"}},
		{"estimate", "network/discovery_quote_public_vm.json", 200, 0, []string{"estimate", "--cli-input-json", `{"PublicIPs":[{"Key":"quoted","Kind":"public-vip","Type":"public-vm","Quantity":3}]}`}},
		{"estimate-unknown", "network/discovery_quote_public_vm.json", 200, 0, []string{"estimate", "--cli-input-json", `{"PublicIPs":[{"Key":"quoted","Kind":"public-vip","Type":"public-vm","Quantity":3},{"Key":"unknown","Kind":"floating-ip","Quantity":2}]}`}},
		{"estimate-partial", "", 403, 1, []string{"estimate", "--allow-partial", "--cli-input-json", `{"PublicIPs":[{"Key":"unknown","Kind":"floating-ip","Quantity":2},{"Key":"failed","Kind":"public-vip","Type":"public-vm","Quantity":3}]}`}},
	}
	for _, c := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(c.name+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){"/v1/price": func(w http.ResponseWriter, _ *http.Request) {
					if c.file != "" {
						testutil.WriteFixture(t, w, "../../testdata/"+c.file)
						return
					}
					w.WriteHeader(c.code)
					_, _ = w.Write([]byte(`{}`))
				}})
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--project-id", "proj-1", "--read-only", "--output", format, "pricing"}
				root.SetArgs(append(args, c.args...))
				err := root.ExecuteContext(computeDiscoveryCLIContext())
				if exitCode(err) != c.exit {
					t.Fatalf("unexpected command exit: %v", err)
				}
				if err != nil {
					printError(stderr, err)
				}
				for suffix, bytes := range map[string][]byte{".golden": stdout.Bytes(), ".stderr.golden": stderr.Bytes()} {
					if err := os.WriteFile(filepath.Join(dir, c.name+"."+format+suffix), bytes, 0o644); err != nil {
						t.Fatal(err)
					}
				}
			})
		}
	}
}
