Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Archive the tag into an empty temporary directory and copy
monitor_goldens_test.go into internal/cli. Set MONITOR_FIXTURE_DIR to the
absolute path of crates/greennode-sdk/tests/fixtures/monitor/, set
MONITOR_GOLDEN_DIR to an empty output directory, and use a temporary HOME
with GOPROXY=off and GOTOOLCHAIN=local. Run `go test ./internal/cli -run
'^TestMonitorReadGoldens$' -count=1` from the archived module. Copy the nine
files into crates/greennode/tests/fixtures/monitor/. The test drives the Go
CLI in read-only mode against loopback responses.

Run `go test ./internal/cli -run '^TestMonitorQuotaGoldens$' -count=1` with
the same settings to generate twelve catalog, snapshot, and partial snapshot
files. The time normalization replaces only the live AsOf field or header.

Run `go test ./internal/cli -run '^TestMonitorSyntheticQuoteGoldens$' -count=1`
with the same settings for the six quote output goldens.

Run `go test ./internal/cli -run '^TestMonitorNotificationGoldens$' -count=1`
with the same settings for the six notification quota and usage goldens.

Run `go test ./internal/cli -run '^TestMonitorNextReadGoldens$' -count=1` to
generate six notification quote and check alarm goldens.

Run `go test ./internal/cli -run '^TestMonitorRemainingGoldens$' -count=1`
with the same settings for the 51 history, alarm, channel, check, location,
log-project, and quota read goldens.

Run `go test ./internal/cli -run '^TestMonitorChannelWriteGoldens$' -count=1`
with TZ=UTC and the same settings for the 12 channel and OTP write goldens.

Run `go test ./internal/cli -run '^TestMonitorCheckWriteGoldens$' -count=1`
with the same settings for the 15 synthetic check write goldens.

Run `go test ./internal/cli -run '^TestMonitorLogAlarmWriteGoldens$' -count=1`
with the same settings for the nine log alarm write goldens.

Run `go test ./internal/cli -run '^TestMonitorLogProjectWriteGoldens$' -count=1`
with the same settings for the six log project write goldens.
