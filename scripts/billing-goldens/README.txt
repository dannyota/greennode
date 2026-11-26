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

For cost explorer and balances, copy cost_goldens_test.go into internal/cli
and run go test ./internal/cli -run '^TestBillingCostGoldens$' -count=1 with
the same environment. The balances case substitutes a long decimal for
cashAvailable. Copy the nine files to crates/greennode/tests/fixtures/billing/.

Generate the resource list goldens and verify the baseline's existing history
goldens with go test ./internal/cli -run '^(TestBillingResourcesGoldens|TestBillingHistoryOutput)$'
-count=1 using the same setup. The resource case substitutes a long decimal
for the first cost. Copy the three generated list-resources files
to crates/greennode/tests/fixtures/billing/. The Rust history tests use the
baseline's existing billing-list-transactions and billing-list-payments goldens
under crates/greennode/tests/fixtures/render/golden/.
