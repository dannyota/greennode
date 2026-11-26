package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestDNSPortGoldens(t *testing.T) {
	for _, test := range []struct {
		command, fixture, route, query string
		flags                          []string
	}{
		{"get-hosted-zone", "get_hosted_zone", "/v1/dns/hosted-zone/zone-1", "", []string{"--hosted-zone-id", "zone-1"}},
		{"get-record", "get_record", "/v1/dns/hosted-zone/zone-1/record/record-1", "", []string{"--hosted-zone-id", "zone-1", "--record-id", "record-1"}},
		{"list-hosted-zones", "list_hosted_zones", "/v1/dns/hosted-zone", "name=example&page=2&size=10", []string{"--name", "example", "--page", "2", "--size", "10"}},
		{"list-records", "list_records", "/v1/dns/hosted-zone/zone-1/record", "name=www", []string{"--hosted-zone-id", "zone-1", "--name", "www"}},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "dns", test.fixture+".json"))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(test.command+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
					test.route: jsonHandler(http.StatusOK, string(body)),
				})
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--output", format, "dns", test.command}
				root.SetArgs(append(args, test.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 {
					t.Fatal("expected successful DNS read")
				}
				method, _ := fixture.methodFor(test.route)
				query, _ := fixture.queryFor(test.route)
				if fixture.requestCount() != 1 || method != http.MethodGet || query != test.query {
					t.Fatal("unexpected DNS request")
				}
				name := test.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("DNS_GOLDEN_DIR"), name), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
