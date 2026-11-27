Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag to an empty temporary directory and copy tagging_goldens_test.go
to its internal/cli directory. Set TAGGING_GOLDEN_DIR to an empty directory,
HOME to a temporary directory, GOPROXY=off and GOTOOLCHAIN=local. Run
 go test ./internal/cli -run '^TestTagging(Read|Write|SystemTag)Goldens$' -count=1
from the archived module. Copy the generated files into
crates/greennode/tests/fixtures/tagging/.
