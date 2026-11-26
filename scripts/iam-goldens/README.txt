Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the pinned tag into an empty temporary directory. Copy
`iam_goldens_test.go` into the archive's `internal/cli` directory and copy
`crates/greennode/tests/fixtures/iam` into a temporary fixture directory.
Set `IAM_GOLDEN_DIR` to that absolute fixture directory and `HOME` to a
separate temporary directory. Point `GOMODCACHE` and `GOCACHE` at the existing
Go caches. Run from the archive with cached dependencies and loopback access.

    GOPROXY=off GOTOOLCHAIN=local go test ./internal/cli -run '^TestIAMPortGoldens$' -count=1

Copy the generated `.golden` files back into `crates/greennode/tests/fixtures/iam`.
