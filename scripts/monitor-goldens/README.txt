Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty temporary directory and copy
monitor_goldens_test.go into internal/cli. Set MONITOR_FIXTURE_DIR to the
absolute path of crates/greennode-sdk/tests/fixtures/monitor/, set
MONITOR_GOLDEN_DIR to an empty output directory, and use a temporary HOME
with GOPROXY=off and GOTOOLCHAIN=local. Run `go test ./internal/cli -run
'^TestMonitorReadGoldens$' -count=1` from the archived module. Copy the nine
files into crates/greennode/tests/fixtures/monitor/. The test drives the Go
CLI in read-only mode against loopback responses.
