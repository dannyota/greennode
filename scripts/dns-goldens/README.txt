Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty temporary directory. Copy dns_goldens_test.go
to its internal/cli directory. Set DNS_GOLDEN_DIR to an existing output
directory and HOME to a temporary directory. Use an existing, populated
GOMODCACHE with GOPROXY=off and GOTOOLCHAIN=local.
Run go test ./internal/cli -run '^TestDNSPortGoldens$' -count=1 from the
archived module. The loopback CLI reads the baseline fixtures. The projection
adapter keeps Go's JSON tags for DNS resource fields and normalizes nil slices.
The JSON goldens use standard two-space indentation for nested resources.
Copy the twelve stdout goldens to crates/greennode/tests/fixtures/dns/.
