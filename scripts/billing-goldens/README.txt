Baseline: vngcloud v1.7.0, commit 8cc8463df39a9c72fa32dcfb359c09618402288f.

Archive the tag into an empty temporary directory. Copy billing_goldens_test.go
into its internal/cli directory. Set BILLING_GOLDEN_DIR to an empty output
directory, HOME to a temporary directory, GOPROXY=off and GOTOOLCHAIN=local.
Run go test ./internal/cli -run '^TestBillingTemplateGoldens$' -count=1 from
the archived module. The test injects a loopback fixture through the baseline's
newSvcRoot helper. Copy the six generated files to
greennode/tests/fixtures/billing/.

For get-current-period-cost, list-budget-thresholds, and list-budget-alerts,
use '^TestBillingBudgetReadGoldens$' instead. The test reads the pinned
baseline's testdata/billing response fixtures and writes nine stdout goldens.
Copy them to crates/greennode/tests/fixtures/billing/. Each case expects exit
zero and empty stderr.
