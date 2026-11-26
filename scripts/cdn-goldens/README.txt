Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty temporary directory. Copy cdn_goldens_test.go
into internal/cli in the archive. Set CDN_GOLDEN_DIR to an empty output
directory, HOME to a temporary home, GOPROXY=off and GOTOOLCHAIN=local.
Run go test ./internal/cli -run '^TestCDNGoldens$' -count=1 from the archive.
The test uses the Go CLI with loopback fixtures and normalizes the public
page's loopback origin. Copy the generated stdout goldens to
crates/greennode/tests/fixtures/cdn/.
