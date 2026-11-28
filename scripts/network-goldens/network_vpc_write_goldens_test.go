package cli

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"danny.vn/vngcloud"
)

type networkVPCWriteCase struct {
	Command  string
	Args     []string
	Requests []struct {
		Method   string
		Path     string
		Status   int
		Response json.RawMessage
		Body     json.RawMessage
	}
}

func TestNetworkVPCWriteGoldens(t *testing.T) {
	dir := os.Getenv("NETWORK_GOLDEN_DIR")
	raw, err := os.ReadFile(filepath.Join(dir, "vpc-writes.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []networkVPCWriteCase
	if err := json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	for _, tc := range cases {
		for _, format := range []string{"json", "table", "text"} {
			t.Run(tc.Command+"/"+format, func(t *testing.T) {
				withCleanEnv(t)
				calls := 0
				transport := saCLITransport(func(r *http.Request) (*http.Response, error) {
					if calls >= len(tc.Requests) {
						t.Fatal("unexpected request")
					}
					want := tc.Requests[calls]
					calls++
					if r.Method != want.Method || r.URL.Host != "hcm-3.console.greennode.ai" ||
						r.URL.RequestURI() != "/vserver/iam-vserver-gateway/v2/proj-1/"+want.Path {
						t.Fatalf("unexpected route: %s %s", r.Method, r.URL.RequestURI())
					}
					var actual, expected any
					if len(want.Body) != 0 {
						if r.Body == nil || json.NewDecoder(r.Body).Decode(&actual) != nil ||
							json.Unmarshal(want.Body, &expected) != nil || !reflect.DeepEqual(actual, expected) {
							t.Fatal("unexpected request body")
						}
					} else if r.Body != nil && r.ContentLength != 0 {
						t.Fatal("unexpected nonempty request body")
					}
					return &http.Response{StatusCode: want.Status, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(string(want.Response))), Request: r}, nil
				})
				withTestOptions(t, vngcloud.WithStaticToken("toy-token"), vngcloud.WithTransport(transport))
				var stdout, stderr strings.Builder
				root := newRootCmd(strings.NewReader(""), &stdout, &stderr)
				root.SetArgs(append([]string{"--region", "hcm-3", "--project-id", "proj-1", "--output", format, "network", tc.Command}, tc.Args...))
				if err := root.ExecuteContext(context.Background()); err != nil {
					t.Fatal(err)
				}
				if calls != len(tc.Requests) || stderr.Len() != 0 {
					t.Fatal("expected all requests and empty stderr")
				}
				if err := os.WriteFile(filepath.Join(dir, tc.Command+"."+format+".golden"), []byte(stdout.String()), 0600); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}
