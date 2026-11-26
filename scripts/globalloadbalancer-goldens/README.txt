Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty temporary directory. Copy
globalloadbalancer_goldens_test.go into its internal/cli directory.
Set GLB_GOLDEN_DIR to an empty output directory, HOME to a temporary home,
GOPROXY=off and GOTOOLCHAIN=local. With the cached Go module directory in
GOMODCACHE, run from the archived module:

    go test ./internal/cli -run '^TestGlobalLoadBalancerReadGoldens$' -count=1

The test runs the complete Go CLI in-process with the ten testdata/glb
responses. Copy the 30 stdout goldens to
crates/greennode/tests/fixtures/globalloadbalancer/.

Copy globalloadbalancer_write_goldens_test.go into the same archive and run:

    go test ./internal/cli -run '^TestGlobalLoadBalancerWriteGoldens$' -count=1

Copy the eight success and partial stdout goldens to the same directory.

Copy globalloadbalancer_child_write_goldens_test.go into the same archive.
Set GLB_CHILD_FIXTURE to the absolute path of child_writes.json under
crates/greennode-sdk/tests/fixtures/globalloadbalancer/ and run:

    go test ./internal/cli -run '^TestGlobalLoadBalancerChildWriteGoldens$' -count=1

Copy the sixteen success and partial stdout goldens to the same directory.
