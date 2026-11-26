package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestVolumeGoldens(t *testing.T) {
	dir := os.Getenv("VOLUME_GOLDEN_DIR")
	if dir == "" {
		t.Fatal("VOLUME_GOLDEN_DIR is required")
	}
	fixtures := os.Getenv("VOLUME_FIXTURE_DIR")
	if fixtures == "" {
		fixtures = filepath.Join("..", "..", "testdata", "volume")
	}
	fixture := func(t *testing.T, name string) string {
		t.Helper()
		body, err := os.ReadFile(filepath.Join(fixtures, name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	const volumePath = "/v2/proj-1/volumes"
	const volumeTypePath = "/v1/proj-1/volume_types"
	const snapshotPath = volumePath + "/volume-1/snapshots"
	type gateway = map[string]func(http.ResponseWriter, *http.Request)
	single := func(path, body string) gateway {
		return gateway{path: jsonHandler(200, body)}
	}
	cases := []struct {
		command string
		flags   string
		routes  gateway
	}{
		{"list-volumes", "", single(volumePath, fixture(t, "list_volumes"))},
		{"get-volume", "--volume-id volume-1", single(volumePath+"/volume-1", fixture(t, "get_volume"))},
		{"get-underlying-volume", "--volume-id volume-1", single(volumePath+"/volume-1/mapping", fixture(t, "get_underlying_volume"))},
		{"list-volumes-by-server", "--server-id server-1", single(volumePath+"/servers/server-1", fixture(t, "list_volumes_by_server"))},
		{"list-volume-type-zones", "", single("/v1/proj-1/volume_type_zones", fixture(t, "list_volume_type_zones"))},
		{"list-volume-types", "", single(volumeTypePath, fixture(t, "list_volume_types"))},
		{"get-volume-type", "--volume-type-id type-1", single(volumeTypePath+"/type-1", fixture(t, "get_volume_type"))},
		{"get-default-volume-type", "--zone-id zone-1", single("/v1/proj-1/volume_default_id", fixture(t, "get_default_volume_type"))},
		{"list-encryption-types", "", single("/v1/proj-1/volumes/encryption_types", fixture(t, "list_encryption_types_keys"))},
		{"list-snapshots", "--volume-id volume-1", single(snapshotPath, fixture(t, "list_snapshots"))},
		{"list-all-snapshots", "", gateway{
			volumePath:   jsonHandler(200, `{"listData":[{"uuid":"volume-1"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}`),
			snapshotPath: jsonHandler(200, `{"items":[{"id":"snapshot-1","volumeId":"volume-1"}],"page":1,"pageSize":10000,"totalPages":1,"totalItems":1}`),
		}},
		{"list-snapshot-backends", "--name HCM-03", single("/vserver/vbackup-gateway/v1/backends", fixture(t, "snapshot_backends"))},
		{"list-snapshot-policies", "--backend-id backend-1", single("/vserver/vbackup-gateway/v1/snapshot-policies", fixture(t, "snapshot_policies"))},
		{"quote-create-volume", "--zone-id zone-1 --size 1 --volume-type-id type-1", single("/v1/price", fixture(t, "discovery_quote_plain"))},
		{"quote-resize-volume", "--volume-id volume-1 --size 10000", gateway{
			volumePath + "/volume-1": jsonHandler(200, fixture(t, "get_volume")),
			"/v1/price":              jsonHandler(200, fixture(t, "discovery_quote_plain")),
		}},
		{"list-volume-prices", "--zone-id zone-1 --encryption none --iops 3000", gateway{
			"/v1/proj-1/zones":                    jsonHandler(200, `{"data":[{"uuid":"zone-1","name":"Disabled","isEnabled":false}]}`),
			"/v1/proj-1/volume_type_zones":        jsonHandler(200, `{"volumeTypeZones":[{"id":"parent-1"}]}`),
			"/v1/proj-1/volumes/encryption_types": jsonHandler(200, `[]`),
			"/v1/proj-1/parent-1/volume_types":    jsonHandler(200, fixture(t, "discovery_types")),
			"/v1/price":                           jsonHandler(200, fixture(t, "discovery_quote_plain")),
		}},
	}
	const detached = `{"data":{"uuid":"volume-1","name":"toy-volume","size":1,"status":"AVAILABLE","volumeTypeId":"type-1","serverId":null,"serverIdList":[]}}`
	const attached = `{"data":{"uuid":"volume-1","name":"toy-volume","size":1,"status":"IN-USE","volumeTypeId":"type-1","serverId":"server-1","serverIdList":["server-1"]}}`
	const emptyInventory = `{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}`
	volumeRoute := func(current string) func(http.ResponseWriter, *http.Request) {
		return func(w http.ResponseWriter, r *http.Request) {
			if r.Method == http.MethodGet {
				jsonHandler(200, current)(w, r)
				return
			}
			jsonHandler(202, `{}`)(w, r)
		}
	}
	writes := []struct {
		command string
		flags   string
		routes  gateway
	}{
		{"attach-volume", "--volume-id volume-1 --server-id server-1 --no-wait", gateway{
			volumePath + "/volume-1":                         jsonHandler(200, detached),
			volumePath + "/volume-1/servers/server-1/attach": jsonHandler(202, `{}`),
		}},
		{"create-volume", "--name toy-volume --zone-id zone-1 --volume-type-id type-1 --size 1 --max-price 3200 --no-wait", gateway{
			volumePath: func(w http.ResponseWriter, r *http.Request) {
				if r.Method == http.MethodGet {
					jsonHandler(200, emptyInventory)(w, r)
					return
				}
				jsonHandler(202, `{"data":{"uuid":"volume-1"}}`)(w, r)
			},
			"/v1/price": jsonHandler(200, fixture(t, "discovery_quote_plain")),
		}},
		{"delete-volume", "--volume-id volume-1 --yes --no-wait", gateway{volumePath + "/volume-1": volumeRoute(detached)}},
		{"detach-volume", "--volume-id volume-1 --server-id server-1 --yes --no-wait", gateway{
			volumePath + "/volume-1":                         jsonHandler(200, attached),
			"/v2/proj-1/servers/server-1":                    jsonHandler(200, `{"data":{"status":"STOPPED","bootVolumeId":"boot-1"}}`),
			volumePath + "/volume-1/servers/server-1/detach": jsonHandler(202, `{}`),
		}},
		{"resize-volume", "--volume-id volume-1 --size 2 --max-price 3200 --yes --no-wait", gateway{
			volumePath + "/volume-1":        jsonHandler(200, detached),
			"/v1/price":                     jsonHandler(200, fixture(t, "discovery_quote_plain")),
			volumePath + "/volume-1/resize": jsonHandler(202, `{}`),
		}},
	}
	if os.Getenv("VOLUME_GOLDEN_WRITES") == "1" {
		cases = writes
	}
	for _, tc := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(tc.routes)
				root, stdout, stderr := newSvcRoot(t, fixture)
				if tc.command == "list-snapshot-backends" || tc.command == "list-snapshot-policies" {
					root, stdout, stderr = newSnapshotReadRoot(t, fixture)
				}
				args := []string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format}
				if os.Getenv("VOLUME_GOLDEN_WRITES") != "1" {
					args = append(args, "--read-only")
				}
				args = append(args, "volume", tc.command)
				root.SetArgs(append(args, strings.Fields(tc.flags)...))
				ctx := context.Background()
				if tc.command == "list-volume-prices" {
					ctx = discoveryTestContext()
				}
				if err := root.ExecuteContext(ctx); err != nil {
					t.Fatal(err)
				}
				path := filepath.Join(dir, tc.command+"."+format+".golden")
				if err := os.WriteFile(path, stdout.Bytes(), 0o600); err != nil {
					t.Fatal(err)
				}
				if tc.command == "list-volume-prices" && format != "json" {
					if err := os.WriteFile(path+".stderr", stderr.Bytes(), 0o600); err != nil {
						t.Fatal(err)
					}
				}
			})
		}
	}
}
