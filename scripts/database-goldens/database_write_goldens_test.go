package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"danny.vn/vngcloud/database"
)

func TestDatabaseWriteGoldens(t *testing.T) {
	const detail = `{"code":200,"data":{"id":"db-1","name":"demo-db","status":"ACTIVE","datastoreType":"MySQL","datastoreVersion":"8.0","quotaPackageId":"1","publicAccess":false,"backupAuto":false,"zoneId":"zone-1","projectId":"project-1","subnetId":"subnet-1","volumeType":"SSD","volumeSize":20,"owner":true,"deleted":false}}`
	var response struct{ Data database.Instance }
	if err := json.Unmarshal([]byte(detail), &response); err != nil {
		t.Fatal(err)
	}
	write := func(name string, data []byte) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(os.Getenv("DATABASE_GOLDEN_DIR"), name), data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	write("create-database-instance.response.json", []byte(detail))
	for _, test := range []struct {
		command string
		result  any
		partial any
	}{
		{"create-database-instance", &database.CreateDatabaseInstanceOutput{Instance: response.Data}, &database.CreateDatabaseInstanceOutput{Instance: database.Instance{ID: "db-1"}}},
		{"delete-database-instance", &database.DeleteDatabaseInstanceOutput{ID: "db-1", Deleted: true}, &database.DeleteDatabaseInstanceOutput{ID: "db-1"}},
	} {
		for _, format := range []string{"json", "table", "text"} {
			var output bytes.Buffer
			if err := renderOutput(&output, format, "", test.result, true); err != nil {
				t.Fatal(err)
			}
			write(test.command+"."+format+".golden", output.Bytes())
		}
		var output bytes.Buffer
		if err := renderOutput(&output, "json", "", test.partial, false); err != nil {
			t.Fatal(err)
		}
		write(test.command+".partial.json.golden", output.Bytes())
	}
}
