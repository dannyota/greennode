package cli

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestContainerRegistryWriteGoldens(t *testing.T) {
	read := func(name string) string {
		t.Helper()
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "containerregistry", name+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(body)
	}
	const empty = `{"data":[],"totalPage":0,"totalItem":0}`
	const path = "registry-secret"
	repo := read("create_repository")
	deleted := read("delete_repository")
	permissions := read("list_permissions")
	cases := []struct {
		command     string
		flags       []string
		routes      map[string]func(http.ResponseWriter, *http.Request)
		count, exit int
	}{
		{"create-repository", []string{"--name", "app-images", "--quota-limit-gb", "20", "--no-wait"}, map[string]func(http.ResponseWriter, *http.Request){
			"/v1/repository": jsonHandler(http.StatusAccepted, repo),
		}, 1, 0},
		{"delete-repository", []string{"--repository-id", "repo-1", "--no-wait", "--yes"}, map[string]func(http.ResponseWriter, *http.Request){
			"/v1/repository/repo-1": func(w http.ResponseWriter, r *http.Request) {
				if r.Method == http.MethodDelete {
					jsonHandler(http.StatusAccepted, deleted)(w, r)
					return
				}
				jsonHandler(http.StatusOK, `{"uuid":"repo-1","imageCount":0}`)(w, r)
			},
		}, 2, 0},
		{"delete-user", []string{"--user-id", "robot-1", "--yes"}, map[string]func(http.ResponseWriter, *http.Request){
			"/v1/user/robot-1": jsonHandler(http.StatusOK, `{}`),
		}, 1, 0},
		{"create-user", []string{"--name", "app-ci", "--secret-file", path, "--query", "User.ID", "--cli-input-json", `{"Permissions":[{"RepositoryID":"repo-1","Actions":["Pull Images"]}]}`}, map[string]func(http.ResponseWriter, *http.Request){
			"/v1/user/permissions": jsonHandler(http.StatusOK, permissions),
			"/v1/user": func(w http.ResponseWriter, r *http.Request) {
				if r.Method == http.MethodPost {
					jsonHandler(http.StatusOK, `{"secretKey":"synthetic-issued-registry-key"}`)(w, r)
					return
				}
				jsonHandler(http.StatusOK, empty)(w, r)
			},
		}, 4, 1},
	}
	dir := os.Getenv("CONTAINERREGISTRY_GOLDEN_DIR")
	for _, tc := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				t.Chdir(t.TempDir())
				fixture := newSvcFixture(tc.routes)
				_, stdout, stderr := newSvcRoot(t, fixture)
				args := []string{"--region", "hcm-3", "--output", format, "containerregistry", tc.command}
				if exit := Main(context.Background(), append(args, tc.flags...), nil, stdout, stderr); exit != tc.exit {
					t.Fatalf("unexpected exit: %d", exit)
				}
				if fixture.requestCount() != tc.count {
					t.Fatalf("wrong request count: %d", fixture.requestCount())
				}
				if err := os.WriteFile(filepath.Join(dir, tc.command+"."+format+".golden"), stdout.Bytes(), 0600); err != nil {
					t.Fatal(err)
				}
				if tc.exit == 1 && format == "json" {
					if err := os.WriteFile(filepath.Join(dir, tc.command+".stderr.golden"), stderr.Bytes(), 0600); err != nil {
						t.Fatal(err)
					}
				}
			})
		}
	}
}
