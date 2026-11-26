Baseline: vngcloud v1.7.0, commit 8cc8463df39a9c72fa32dcfb359c09618402288f.

Archive the baseline into a temporary directory and copy vks_goldens_test.go
into its internal/cli directory. Set VKS_GOLDEN_DIR to an empty output
directory, HOME to a temporary directory, GOPROXY=off and GOTOOLCHAIN=local.
Run go test ./internal/cli -run '^TestVKS(Delete)?Goldens$' -count=1 from the archive.
The tests run the baseline CLI in process against loopback fixtures or a fake operation.
Copy the stdout goldens into crates/greennode/tests/fixtures/vks/.
