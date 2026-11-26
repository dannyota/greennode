Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the pinned tag into an empty temporary directory and copy
compute_goldens_test.go into its internal/cli directory. Set COMPUTE_GOLDEN_DIR
to an empty output directory, COMPUTE_WIRE_DIR to the absolute path of
crates/greennode-sdk/tests/fixtures/compute and HOME to a temporary directory. With cached
Go dependencies, GOPROXY=off and GOTOOLCHAIN=local, run
`go test ./internal/cli -run '^TestCompute(Price)?PortGoldens$' -count=1` from the archive.
Copy the generated stdout and discovery stderr files into crates/greennode/tests/fixtures/compute/.
