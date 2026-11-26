package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestContainerRegistryGoldens(t *testing.T) {
	cases := []struct {
		command, fixture, path string
		flags                  []string
	}{
		{"list-repositories", "list_repositories", "/v1/repository", nil},
		{"get-repository", "get_repository", "/v1/repository/repo-1", []string{"--repository-id", "repo-1"}},
		{"list-users", "list_users", "/v1/user", nil},
		{"list-repository-users", "list_users", "/v1/repository/repo-1/user", []string{"--repository-id", "repo-1"}},
		{"list-permissions", "list_permissions", "/v1/user/permissions", nil},
	}
	for _, tc := range cases {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "containerregistry", tc.fixture+".json"))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){tc.path: jsonHandler(http.StatusOK, string(body))})
				root, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--output", format, "containerregistry", tc.command}
				root.SetArgs(append(args, tc.flags...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if fixture.requestCount() != 1 || stderr.Len() != 0 {
					t.Fatal("expected one request and empty stderr")
				}
				if err := os.WriteFile(filepath.Join(os.Getenv("CONTAINERREGISTRY_GOLDEN_DIR"), tc.command+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
