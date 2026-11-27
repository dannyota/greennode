Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty directory and copy network_goldens_test.go into
internal/cli. Set NETWORK_GOLDEN_DIR to the absolute path of
crates/greennode/tests/fixtures/network. Use a temporary HOME, set GOPROXY=off
and GOTOOLCHAIN=local. Set NETWORK_FIXTURE_DIR to the absolute path of
crates/greennode-sdk/tests/fixtures/network for the purchase quotes. Then run
from the archived module:

  go test ./internal/cli -run '^TestNetwork(Goldens|InventoryGoldens|PriceGoldens|PurchaseQuoteGoldens)$' -count=1

The generator runs the Go CLI in process. An injected transport checks each
product origin and request path and serves the archived synthetic fixture.
