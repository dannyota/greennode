Baseline: vngcloud v1.7.0, commit 8cc8463df39a9c72fa32dcfb359c09618402288f.

Archive the tag into an empty directory and copy portal_goldens_test.go into
internal/cli. Set PORTAL_GOLDEN_DIR to crates/greennode/tests/fixtures/portal/
using its absolute path. Use a temporary HOME, set GOPROXY=off and
GOTOOLCHAIN=local, then run the following command from the archived module:

  go test ./internal/cli -run '^TestPortalGoldens$' -count=1

The test runs the baseline CLI in-process against its loopback fixture.
