package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestTaggingReadGoldens(t *testing.T) {
	body, err := os.ReadFile("../../testdata/tagging/list_resource_tags.json")
	if err != nil {
		t.Fatal(err)
	}
	for _, format := range []string{"json", "table", "text"} {
		t.Run(format, func(t *testing.T) {
			fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){"/v2/project-1/tag/resource/res-1": jsonHandler(http.StatusOK, string(body))})
			root, stdout, stderr := newSvcRoot(t, fixture)
			root.SetArgs([]string{"--region", "hcm-3", "--project-id", "project-1", "--output", format, "tagging", "list-resource-tags", "--resource-id", "res-1"})
			if err := root.ExecuteContext(context.Background()); err != nil {
				t.Fatal(err)
			}
			if fixture.requestCount() != 1 || stderr.Len() != 0 {
				t.Fatal("expected one request and empty stderr")
			}
			if err := os.WriteFile(filepath.Join(os.Getenv("TAGGING_GOLDEN_DIR"), "list-resource-tags."+format+".golden"), stdout.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		})
	}
}
