package cli

import (
    "context"
    "net/http"
    "os"
    "path/filepath"
    "testing"
)

func TestPortalGoldens(t *testing.T) {
    for _, tc := range []struct { command, fixture, path string; args []string }{
        {"get-user-info", "get_user_info.json", "/v1/users/info", nil},
        {"list-zones", "list_zones.json", "/v1/proj-1/zones", nil},
        {"list-quota-used", "list_quota_used.json", "/v2/proj-1/quotas/quotaUsed", nil},
        {"get-quota", "list_quota_used.json", "/v2/proj-1/quotas/quotaUsed", []string{"--name", "<name>"}},
        {"get-tag-quota", "get_tag_quota.json", "/v2/proj-1/tag/quota", nil},
    } {
        body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "portal", tc.fixture))
        if err != nil { t.Fatal(err) }
        for _, format := range []string{"json", "table", "text"} {
            t.Run(tc.command+"/"+format, func(t *testing.T) {
                fixture := newSvcFixture(map[string]func(http.ResponseWriter, *http.Request){tc.path: jsonHandler(http.StatusOK, string(body))})
                root, stdout, stderr := newSvcRoot(t, fixture)
                args := []string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "portal", tc.command}
                root.SetArgs(append(args, tc.args...))
                if err := root.ExecuteContext(context.Background()); err != nil { t.Fatal(err) }
                if fixture.requestCount() != 1 || stderr.Len() != 0 { t.Fatal("expected one request and empty stderr") }
                if err := os.WriteFile(filepath.Join(os.Getenv("PORTAL_GOLDEN_DIR"), tc.command+"."+format+".golden"), stdout.Bytes(), 0600); err != nil { t.Fatal(err) }
            })
        }
    }
}
