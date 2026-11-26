Baseline: vngcloud v1.7.0, commit 8cc8463df39a9c72fa32dcfb359c09618402288f.

Archive the tag into an empty temporary directory. Copy backup_goldens_test.go
to its internal/cli directory. Set BACKUP_GOLDEN_DIR to an existing output
directory and HOME to a temporary directory. Use an existing, populated
GOMODCACHE and set GOPROXY=off and GOTOOLCHAIN=local.
Run go test ./internal/cli -run '^TestBackupPortGoldens$' -count=1 from the
archived module. The test uses the baseline's loopback backupCLI helper and
sanitized Backup Center fixtures. Copy the nine stdout goldens to
crates/greennode/tests/fixtures/backup/.
