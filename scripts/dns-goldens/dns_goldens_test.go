package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"danny.vn/vngcloud/dns"
	"danny.vn/vngcloud/internal/core"
)

func TestDNSPortGoldens(t *testing.T) {
	for index := range dnsOps {
		if dnsOps[index].kind == kindRead {
			original := dnsOps[index].render
			dnsOps[index].render = renderDNSPort
			t.Cleanup(func() { dnsOps[index].render = original })
		}
	}
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
				data := stdout.Bytes()
				if format == "json" {
					var pretty bytes.Buffer
					if err := json.Indent(&pretty, data, "", "  "); err != nil {
						t.Fatal(err)
					}
					data = pretty.Bytes()
				}
				name := test.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("DNS_GOLDEN_DIR"), name), data, 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func renderDNSPort(w io.Writer, format, query string, output any) error {
	switch value := output.(type) {
	case *dns.GetHostedZoneOutput:
		data, err := json.Marshal(dnsPortZone(value.HostedZone))
		if err != nil {
			return err
		}
		output = struct{ HostedZone json.RawMessage }{data}
	case *dns.GetRecordOutput:
		data, err := json.Marshal(dnsPortRecord(value.Record))
		if err != nil {
			return err
		}
		output = struct{ Record json.RawMessage }{data}
	case *dns.ListHostedZonesOutput:
		page, err := dnsPortPage(value, dnsPortZone)
		if err != nil {
			return err
		}
		output = page
	case *dns.ListRecordsOutput:
		page, err := dnsPortPage(value, dnsPortRecord)
		if err != nil {
			return err
		}
		output = page
	}
	return renderOutput(w, format, query, output, false)
}

func dnsPortPage[T any](page *core.PagedList[T], normalize func(T) T) (*core.PagedList[json.RawMessage], error) {
	items := make([]json.RawMessage, 0, len(page.Items))
	for _, item := range page.Items {
		data, err := json.Marshal(normalize(item))
		if err != nil {
			return nil, err
		}
		items = append(items, data)
	}
	return core.NewPagedList(items, page.Page, page.PageSize, page.TotalPage, page.TotalItem), nil
}

func dnsPortZone(zone dns.HostedZone) dns.HostedZone {
	if zone.AssociatedVPCIDs == nil {
		zone.AssociatedVPCIDs = []string{}
	}
	if zone.AssocVPCMapRegion == nil {
		zone.AssocVPCMapRegion = []dns.VPCMapRegion{}
	}
	return zone
}

func dnsPortRecord(record dns.Record) dns.Record {
	if record.Value == nil {
		record.Value = []dns.RecordValue{}
	}
	return record
}
