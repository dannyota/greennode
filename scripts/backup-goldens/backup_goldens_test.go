package cli

import (
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

func TestBackupPortGoldens(t *testing.T) {
	for _, tc := range []struct {
		command, fixture, target string
		flags                    []string
	}{
		{"get-policy", "get-policy", "/vbackup-gateway/v1/backup-policies/synthetic", []string{"--policy-id", "synthetic"}},
		{"list-backends", "backends", "/vbackup-gateway/v1/backends", nil},
		{"list-policies", "policies", "/vbackup-gateway/v1/backup-policies?page=1&size=200", nil},
	} {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "backup", tc.fixture+".json"))
		if err != nil {
			t.Fatal(err)
		}
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.command+"/"+format, func(t *testing.T) {
				calls := 0
				run := backupCLI(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					calls++
					if r.Method != http.MethodGet || r.URL.RequestURI() != tc.target || r.Header.Get("projectId") != "" {
						t.Error("unexpected request")
					}
					jsonHandler(http.StatusOK, string(body))(w, r)
				}))
				args := []string{"--profile", "agent", "--region", "han-1", "--output", format, "backup", tc.command}
				code, stdout, stderr := run(append(args, tc.flags...)...)
				if code != 0 || stderr != "" || calls != 1 {
					t.Fatalf("expected success and a single request, got exit %d and %d requests", code, calls)
				}
				name := tc.command + "." + format + ".golden"
				if err := os.WriteFile(filepath.Join(os.Getenv("BACKUP_GOLDEN_DIR"), name), []byte(stdout), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
