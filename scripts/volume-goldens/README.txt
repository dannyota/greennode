Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty temporary directory. Copy volume_goldens_test.go
into its internal/cli directory. Set VOLUME_GOLDEN_DIR to an empty output
directory, HOME to a temporary directory, GOPROXY=off and GOTOOLCHAIN=local.
Run go test ./internal/cli -run '^TestVolumeReadGoldens$' -count=1 from the
archived module. Copy the stdout goldens and the discovery stderr goldens to
crates/greennode/tests/fixtures/volume/. The tests use loopback gateways and
inject a clock for the price discovery completion timestamp.
