Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the pinned tag into an empty temporary directory and copy
both Go test files into its internal/cli directory. Set LOADBALANCER_GOLDEN_DIR
to an empty output directory, LOADBALANCER_WIRE_DIR to the absolute path of
crates/greennode-sdk/tests/fixtures/loadbalancer and HOME to a temporary directory. With cached
Go dependencies, GOPROXY=off and GOTOOLCHAIN=local, run
`go test ./internal/cli -run '^TestLoadBalancerPort(Goldens|Errors|Deletes|CreateWrites)$' -count=1` from the archive.
Copy the generated stdout and stderr files into crates/greennode/tests/fixtures/loadbalancer/.
