Baseline: vngcloud v1.7.0, commit 8cc8463df39a9c72fa32dcfb359c09618402288f.

Archive the tag to an empty temporary directory and copy tagging_goldens_test.go
to its internal/cli directory. Set TAGGING_GOLDEN_DIR to an empty directory,
HOME to a temporary directory, GOPROXY=off and GOTOOLCHAIN=local. Run
 go test ./internal/cli -run '^TestTaggingReadGoldens$' -count=1
from the archived module. Copy the three generated files into
crates/greennode/tests/fixtures/tagging/.
