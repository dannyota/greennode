# Query fixture provenance

The files in `compliance/` are copied unchanged from
[go-jmespath v0.4.0](https://github.com/jmespath/go-jmespath/tree/v0.4.0/compliance),
Copyright 2015 James Saryerwinnie, Apache-2.0. The license is in
`crates/greennode/licenses/go-jmespath-LICENSE`.

The Go module checksum is `h1:BEgLn5cpjn8UN1mAw4NjwDrS35OdebyEtFe+9YPoQUg=`. The
fixtures were copied from the local Go module cache.

`differential.json` contains synthetic cases from `scripts/query-oracle.go`,
generated with Go 1.27.4 and the same cached module. Run
`go -C "$HOME/go/pkg/mod/github.com/jmespath/go-jmespath@v0.4.0" run "$PWD/scripts/query-oracle.go" > crates/greennode/tests/fixtures/query/differential.json`
from the repository root. Cases excluded from replay carry their reason in the
fixture. Go map traversal is unspecified; object traversal cases use one key.
