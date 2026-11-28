Baseline: vngcloud v1.8.0, commit 85458dfb6b373116be410ec8be845257ad545f96.

Copy the pinned source into an empty temporary directory, then copy
catalogue_goldens_test.go into its internal/cli directory. Set HOME to an
empty temporary directory and PRICE_CATALOGUE_GOLDEN to the absolute path
of an output file. Use the go.mod toolchain (Go 1.27.4) and cached dependencies. Use the
toolchain defaults from the baseline release build. Leave GOEXPERIMENT unset
so encoding/json uses its default v2-backed implementation.

    GOPROXY=off GOTOOLCHAIN=local go test ./internal/cli -run '^TestPriceCataloguePortGoldens$' -count=1

Copy the output to crates/greennode-sdk/tests/fixtures/price_catalogue/go-v1.8.0.json.
The generator calls the Go SDK and its catalogue and quote validators through
loopback HTTP. It records request paths and raw queries, response outcomes,
and successful item values. The Rust corpus test ignores key case and
compares decimal values exactly because the SDK exports Go field names while some Go
model JSON tags use wire names. The estimate rows include the full quote
breakdown. The loopback servers expand the large flavor page and surrogate
escape or byte-count padding from compact fixture records.

See crates/greennode-sdk/licenses/go-NOTICE and go-LICENSE for scanner
provenance and BSD-3-Clause terms.
