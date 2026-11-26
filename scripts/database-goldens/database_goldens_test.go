package cli

import (
	"bytes"
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"danny.vn/vngcloud"
)

func TestDatabaseGoldens(t *testing.T) {
	cases := []struct {
		command, body string
		flags         []string
	}{
		{"get-database-instance", `{"code":200,"data":{"id":"db_one","name":"db","status":"ACTIVE","volumeSize":10,"publicAccess":false,"replicas":null}}`, []string{"--id", "db_one"}},
		{"list-database-engine-versions", `{"code":200,"data":[{"name":"MySQL","type":"mysql","versionName":"MySQL 8","version":"8.0","licenseName":"GPL"}]}`, nil},
		{"list-database-zones", `{"code":200,"data":[{"uuid":"zone-1","name":"Zone A","status":"ACTIVE","zoneType":"LOCAL"}]}`, nil},
		{"list-database-flavors", `{"code":200,"data":[{"id":1,"name":"small","flavorId":"f-1","vcpus":2,"ram":4,"monthlyCost":123.45}]}`, []string{"--engine", "mysql", "--engine-version", "8.0", "--zone-id", "zone-1"}},
		{"list-database-storage-types", `{"code":200,"data":{"projectId":"project-1","data":[{"id":1,"type":"SSD","minVolumeSize":10,"maxVolumeSize":100}]}}`, []string{"--zone-id", "zone-1"}},
		{"list-database-networks", `{"code":200,"data":[{"uuid":"network-1","displayName":"Network","subnets":[{"uuid":"subnet-1","networkUuid":"network-1","name":"Subnet"}]}]}`, []string{"--zone-id", "zone-1"}},
		{"list-database-instances", `{"code":200,"data":{"projectId":"project-1","data":[{"id":"instance-1","name":"db","status":"ACTIVE","publicAccess":false,"volumeSize":10,"replicas":null}],"pageObject":{"number":1,"size":100,"totalElements":1,"totalPages":1,"maxSize":100}}}`, nil},
	}
	for _, tc := range cases {
		if err := os.WriteFile(filepath.Join(os.Getenv("DATABASE_GOLDEN_DIR"), tc.command+".response.json"), []byte(tc.body), 0600); err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				withCleanEnv(t)
				count := 0
				server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { count++; _, _ = w.Write([]byte(tc.body)) }))
				defer server.Close()
				withTestOptions(t, vngcloud.WithStaticToken("toy-bearer"), vngcloud.WithEndpointOverrides(vngcloud.EndpointOverrides{Database: server.URL}))
				stdout, stderr := &bytes.Buffer{}, &bytes.Buffer{}
				root := newRootCmd(strings.NewReader(""), stdout, stderr)
				args := []string{"--region", "hcm-3", "--output", format, "database", tc.command}
				root.SetArgs(append(args, tc.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if count != 1 || stderr.Len() != 0 {
					t.Fatal("expected one request and empty stderr")
				}
				name := tc.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("DATABASE_GOLDEN_DIR"), name), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
