Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the pinned tag into an empty temporary directory. Copy
`iam_goldens_test.go` into the archive's `internal/cli` directory and copy
`crates/greennode/tests/fixtures/iam` into a temporary fixture directory.
Set `IAM_GOLDEN_DIR` to that absolute fixture directory and `HOME` to a
separate temporary directory. Point `GOMODCACHE` and `GOCACHE` at the existing
Go caches. Run from the archive with cached dependencies and loopback access.

    GOPROXY=off GOTOOLCHAIN=local go test ./internal/cli -run '^TestIAMPortGoldens$' -count=1

Copy the generated `.golden` files back into `crates/greennode/tests/fixtures/iam`.

For group writes, also copy `iam_writes_goldens_test.go` into `internal/cli`.
It uses `writes/cases.json` from the same fixture directory and writes the
results beside it. Run:

    GOPROXY=off GOTOOLCHAIN=local go test ./internal/cli -run '^TestIAMWritePortGoldens$' -count=1

For service-account writes, also copy `iam_service_account_writes_goldens_test.go`
into `internal/cli`. It reads and writes `service-account-writes/` in the same
fixture directory. Run:

    GOPROXY=off GOTOOLCHAIN=local go test ./internal/cli -run '^TestIAMServiceAccountWritePortGoldens$' -count=1
