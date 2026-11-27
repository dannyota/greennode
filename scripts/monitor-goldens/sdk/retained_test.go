package monitor

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"

	"danny.vn/vngcloud/internal/core"
)

func TestSyntheticRetainedMutationGoldens(t *testing.T) {
	load := func(file string) string {
		t.Helper()
		data, err := os.ReadFile(filepath.Join(os.Getenv("MONITOR_FIXTURE_DIR"), file+".json"))
		if err != nil {
			t.Fatal(err)
		}
		return string(data)
	}
	const quota = "/billing-api/v1/synthetic/quota"
	const classes = "/billing-api/v2/synthetic/quota-class"
	const expiry = "2099-02-01T00:00:00Z"
	for _, test := range []struct{ name, action, mutation string }{
		{"ensure-after-create", "create", "/billing-api/v2/synthetic/quota"},
		{"renew-after-auto-renew", "auto-renew", quota + "/quota-1/update-auto-renew"},
		{"ensure-after-renew", "renew", "/billing-api/v2/synthetic/quota/extend-time"},
		{"ensure-after-recover", "recover", "/billing-api/v2/trash/synthetic/quota"},
	} {
		t.Run(test.name, func(t *testing.T) {
			current := load("synthetic_quote_renewal")
			if test.action == "recover" {
				current = strings.ReplaceAll(strings.ReplaceAll(current, "ACTIVE", "IN-TRASH"), "2099-", "2000-")
			}
			var sent atomic.Bool
			client := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.Method == http.MethodPost && r.URL.Path == test.mutation {
					if sent.Swap(true) {
						t.Error("fixture received a second mutation")
					}
					if test.action == "auto-renew" {
						w.WriteHeader(http.StatusServiceUnavailable)
						return
					}
					_, _ = w.Write([]byte(`{"orderId":"synth-order"}`))
					return
				}
				var reply string
				switch r.URL.Path {
				case quota:
					if sent.Load() {
						w.WriteHeader(http.StatusForbidden)
						return
					}
					if test.action == "create" {
						w.WriteHeader(http.StatusNotFound)
						_, _ = w.Write([]byte(load("synthetic_quote_absent")))
						return
					}
					reply = current
				case "/billing-api/v2/synthetic/quota-1/quota-detail":
					reply = load("quota_quota_detail")
				case classes:
					reply = load("quota_classes")
				case classes + "/class-1/packages":
					reply = load("quota_class_packages")
				case classes + "/class-2/packages":
					reply = "[]"
				case "/billing-api/v1/settings":
					reply = load("quota_settings")
				case "/billing-api/v2/synthetic/prices/created-price":
					reply = `{"optimumPrice":0}`
				default:
					t.Errorf("unexpected fixture route %s", r.URL.Path)
					w.WriteHeader(http.StatusNotFound)
					return
				}
				_, _ = w.Write([]byte(reply))
			}))
			ctx := context.Background()
			var originErr error
			var out any
			var err error
			switch test.action {
			case "create":
				_, originErr = client.CreateSyntheticQuota(ctx, &CreateSyntheticQuotaInput{ClassID: "class-1", PackageID: "unit_1", Quantity: 10, NoWait: true})
				if originErr != nil {
					t.Fatal("fixture did not accept creation")
				}
				out, err = client.EnsureSyntheticQuota(ctx, &EnsureSyntheticQuotaInput{})
			case "auto-renew":
				enabled := true
				_, originErr = client.SetSyntheticQuotaAutoRenew(ctx, &SetSyntheticQuotaAutoRenewInput{Enabled: &enabled})
				if !errors.Is(originErr, core.ErrNotSettled) {
					t.Fatal("fixture did not retain the auto-renew attempt")
				}
				out, err = client.RenewSyntheticQuota(ctx, &RenewSyntheticQuotaInput{ExpectedEndTime: expiry})
			case "renew":
				_, originErr = client.RenewSyntheticQuota(ctx, &RenewSyntheticQuotaInput{ExpectedEndTime: expiry, NoWait: true})
				if originErr != nil {
					t.Fatal("fixture did not accept the standalone renewal")
				}
				out, err = client.EnsureSyntheticQuota(ctx, &EnsureSyntheticQuotaInput{ExpectedQuotaID: "quota-1"})
			case "recover":
				_, originErr = client.RecoverSyntheticQuota(ctx, &RecoverSyntheticQuotaInput{ExpectedQuotaID: "quota-1", NoWait: true})
				if originErr != nil {
					t.Fatal("fixture did not accept the standalone recovery")
				}
				out, err = client.EnsureSyntheticQuota(ctx, &EnsureSyntheticQuotaInput{ExpectedQuotaID: "quota-1"})
			}
			if !sent.Load() || !errors.Is(err, core.ErrNotSettled) {
				t.Fatal("fixture did not produce an unresolved cross-operation outcome")
			}
			data, err := json.MarshalIndent(out, "", "  ")
			if err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(os.Getenv("MONITOR_GOLDEN_DIR"), test.name+".json"), append(data, '\n'), 0600); err != nil {
				t.Fatal(err)
			}
		})
	}
}
