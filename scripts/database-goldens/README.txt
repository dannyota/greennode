Baseline: vngcloud v1.7.0, commit 8cc8463df39a9c72fa32dcfb359c09618402288f.

Archive the tag into an empty temporary directory. Copy database_goldens_test.go
into its internal/cli directory. Set DATABASE_GOLDEN_DIR to an empty output
directory, HOME to a temporary directory, GOPROXY=off and GOTOOLCHAIN=local.
Run go test ./internal/cli -run '^TestDatabaseGoldens$' -count=1 from the
archived module. The test uses a loopback server and synthetic responses. Copy
the generated response fixtures and stdout goldens to
crates/greennode/tests/fixtures/database/.
