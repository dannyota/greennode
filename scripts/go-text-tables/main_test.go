package main

import (
	"bytes"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func TestSimpleFoldTableMatchesGeneratedOutput(t *testing.T) {
	_, source, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot locate the generator test")
	}
	fixture := filepath.Join(filepath.Dir(source), "..", "..", "crates", "greennode-sdk", "src", "go_text", "simple_fold.txt")
	checkedIn, err := os.ReadFile(fixture)
	if err != nil {
		t.Fatal(err)
	}
	generated := filepath.Join(t.TempDir(), "simple_fold.txt")
	write(generated, tableHeader()+"\n"+simpleFoldColumns, simpleFold())
	output, err := os.ReadFile(generated)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(output, checkedIn) {
		t.Fatal("the checked-in table differs from Go's generated simple-fold data")
	}
}
