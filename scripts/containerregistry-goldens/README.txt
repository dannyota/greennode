Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty temporary directory and copy
containerregistry_goldens_test.go to internal/cli. Set HOME to a temporary
directory, GOPROXY=off, GOTOOLCHAIN=local, and CONTAINERREGISTRY_GOLDEN_DIR
to an empty output directory. Run go test ./internal/cli -run
'^TestContainerRegistryGoldens$' -count=1. Copy the 15 generated files to
crates/greennode/tests/fixtures/containerregistry/.
