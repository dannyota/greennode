package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestProjectListGoldens(t *testing.T) {
	body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "project", "list_projects.json"))
	if err != nil {
		t.Fatal(err)
	}
	for _, format := range []string{"json", "table", "text"} {
		t.Run(format, func(t *testing.T) {
			fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){
				"/v1/projects": jsonHandler(http.StatusOK, string(body)),
			})
			root, stdout, stderr := newSvcRoot(t, fixture)
			root.SetArgs([]string{"--region", "hcm-3", "--output", format, "project", "list-projects"})
			if err := root.ExecuteContext(context.Background()); err != nil {
				t.Fatal(err)
			}
			if fixture.requestCount() != 1 || stderr.Len() != 0 {
				t.Fatal("expected one request and empty stderr")
			}
			name := "list-projects." + format + ".golden"
			if err := os.WriteFile(filepath.Join(os.Getenv("PROJECT_GOLDEN_DIR"), name), stdout.Bytes(), 0600); err != nil {
				t.Fatal(err)
			}
		})
	}
}
