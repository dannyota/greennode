package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"strings"
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

func TestDNSWritePortGoldens(t *testing.T) {
	load := func(name string) string {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "dns", name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	createdZone, createdRecord := load("create_hosted_zone"), load("create_record")
	zone, record := load("get_hosted_zone"), load("get_record")
	snapshot := strings.ReplaceAll(createdZone, "CREATING", "ACTIVE")
	replaced := strings.ReplaceAll(snapshot, "<description>", "new description")
	const zones = "/v1/dns/hosted-zone"
	const zoneDetail = zones + "/zone-1"
	const records = zoneDetail + "/record"
	const recordDetail = records + "/record-1"
	type reply struct {
		method, route string
		status        int
		body          string
	}
	for _, test := range []struct {
		command string
		flags   []string
		replies []reply
	}{
		{"create-hosted-zone", []string{"--domain-name", "example.test", "--vpc-ids", "vpc-1", "--vpc-ids", "vpc-2"}, []reply{{http.MethodPost, zones, 200, createdZone}}},
		{"update-hosted-zone", []string{"--hosted-zone-id", "zone-1", "--description", "new description"}, []reply{{http.MethodGet, zoneDetail, 200, snapshot}, {http.MethodPut, zoneDetail, 204, ""}, {http.MethodGet, zoneDetail, 200, replaced}}},
		{"delete-hosted-zone", []string{"--hosted-zone-id", "zone-1", "--yes"}, []reply{{http.MethodGet, zoneDetail, 200, zone}, {http.MethodDelete, zoneDetail, 204, ""}}},
		{"create-record", []string{"--hosted-zone-id", "zone-1", "--type", "TXT", "--cli-input-json", `{"Values":[{"Value":"sample text","Weight":0}]}`}, []reply{{http.MethodGet, zoneDetail, 200, zone}, {http.MethodPost, records, 200, createdRecord}}},
		{"update-record", []string{"--hosted-zone-id", "zone-1", "--record-id", "record-1", "--ttl", "0", "--sticky-session=false", "--cli-input-json", `{"TTL":300,"Values":[]}`}, []reply{{http.MethodGet, zoneDetail, 200, zone}, {http.MethodPut, recordDetail, 204, ""}, {http.MethodGet, recordDetail, 200, record}}},
		{"delete-record", []string{"--hosted-zone-id", "zone-1", "--record-id", "record-1", "--yes"}, []reply{{http.MethodGet, zoneDetail, 200, zone}, {http.MethodDelete, recordDetail, 204, ""}}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(test.command+"/"+format, func(t *testing.T) {
				index := 0
				handle := func(w http.ResponseWriter, req *http.Request) {
					if index == len(test.replies) || req.Method != test.replies[index].method || req.URL.Path != test.replies[index].route {
						t.Error("unexpected DNS write step")
						w.WriteHeader(http.StatusBadRequest)
						return
					}
					step := test.replies[index]
					index++
					jsonHandler(step.status, step.body)(w, req)
				}
				routes := map[string]func(http.ResponseWriter, *http.Request){}
				for _, reply := range test.replies {
					routes[reply.route] = handle
				}
				fixture := newSvcFixture(routes)
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--output", format, "dns", test.command, "--no-wait"}
				root.SetArgs(append(args, test.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil || stderr.Len() != 0 || fixture.requestCount() != len(test.replies) {
					t.Fatalf("expected successful DNS write: %v", err)
				}
				name := test.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("DNS_GOLDEN_DIR"), name), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
