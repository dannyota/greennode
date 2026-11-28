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

For the six budget and threshold writes, use '^TestBillingBudgetWriteGoldens$'
with billing_goldens_test.go and the same setup. Copy the eighteen files to
crates/greennode/tests/fixtures/billing/.

For the response validation corpus, use vngcloud v1.8.0, commit
85458dfb6b373116be410ec8be845257ad545f96, with Go 1.27.4 and its default
JSON implementation. Copy billing_goldens_test.go into the archived module's
internal/cli directory and run go test ./internal/cli
-run '^TestBillingValidationGoldens$' -count=1 with the same offline setup.
Copy validation-go-v1.8.0.json to crates/greennode/tests/fixtures/billing/.
The generator runs all 17 billing commands against loopback responses and
records full stdout, exit codes, and Go CLI error diagnostics. The generator
and Rust replay default to the toy bearer test-token. Cases can override it
to exercise short-credential redaction.

For malformed response strings, use the same v1.8.0 and Go 1.27.4 setup.
Copy lossy_goldens_test.go into internal/cli and run go test ./internal/cli
-run '^TestBillingLossyGoldens$' -count=1. Copy lossy-go-v1.8.0.json to
crates/greennode/tests/fixtures/billing/. The generator verifies that Go
preserves invalid UTF-8 in direct RawMessage JSON. The Rust test expects
U+FFFD for invalid bytes and lone surrogate escapes in direct raw JSON.
It compares the agreeing cases with the Go goldens byte for byte.
