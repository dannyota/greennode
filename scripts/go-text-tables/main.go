package main

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"html"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
)

func must(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func charValue(e ast.Expr) rune {
	lit := e.(*ast.BasicLit)
	r, _, _, err := strconv.UnquoteChar(lit.Value[1:len(lit.Value)-1], '\'')
	must(err)
	return r
}

func parseFile(path string) *ast.File {
	f, err := parser.ParseFile(token.NewFileSet(), path, nil, 0)
	must(err)
	return f
}

func entities(dir string) []string {
	var rows []string
	ast.Inspect(parseFile(filepath.Join(dir, "entity.go")), func(n ast.Node) bool {
		lit, ok := n.(*ast.CompositeLit)
		if !ok {
			return true
		}
		if _, ok := lit.Type.(*ast.MapType); !ok {
			return true
		}
		for _, el := range lit.Elts {
			kv := el.(*ast.KeyValueExpr)
			name, err := strconv.Unquote(kv.Key.(*ast.BasicLit).Value)
			must(err)
			switch v := kv.Value.(type) {
			case *ast.BasicLit:
				rows = append(rows, fmt.Sprintf("%s %X", name, charValue(v)))
			case *ast.CompositeLit:
				rows = append(rows, fmt.Sprintf("%s %X %X", name, charValue(v.Elts[0]), charValue(v.Elts[1])))
			}
		}
		return false
	})
	sort.Strings(rows)
	return rows
}

func replacements(dir string) []string {
	var rows []string
	ast.Inspect(parseFile(filepath.Join(dir, "escape.go")), func(n ast.Node) bool {
		spec, ok := n.(*ast.ValueSpec)
		if !ok || spec.Names[0].Name != "replacementTable" {
			return true
		}
		for _, el := range spec.Values[0].(*ast.CompositeLit).Elts {
			rows = append(rows, fmt.Sprintf("%X", charValue(el)))
		}
		return false
	})
	return rows
}

func printable() []string {
	var rows []string
	start := rune(-1)
	for r := rune(0); r <= 0x10FFFF+1; r++ {
		p := r <= 0x10FFFF && strconv.IsPrint(r)
		if p && start < 0 {
			start = r
		}
		if !p && start >= 0 {
			rows = append(rows, fmt.Sprintf("%X %X", start, r-1))
			start = -1
		}
	}
	return rows
}

func write(path, header string, rows []string) {
	must(os.WriteFile(path, []byte(header+"\n"+strings.Join(rows, "\n")+"\n"), 0o644))
}

type pair struct {
	In  string `json:"in"`
	Out string `json:"out"`
}

func htmlCorpus(dir string) []pair {
	inputs := []string{
		"", "&", "&&", "&#", "&#1", "&#12", "&#x", "&#x1", "&#xZ;", "&#;", "&#x;",
		"&lt", "&lt;", "&lt1.2.3.0/24", "&ltx", "&amp", "&ampx", "&amp;amp;", "&AMP", "&notit;", "&notin;", "&notin",
		"&hairsp;", "&hairsp", "&thinsp;", "&nbsp;", "&nbsp", "&Tab;", "&NewLine;", "&period;", "&sol;", "&colon;",
		"&quot;", "&quot", "&apos;", "&apos", "&copy", "&copy;", "&copyright", "&copyx;", "&Aacute", "&aacutex",
		"&#x85;", "&#133;", "&#x80;", "&#x9F;", "&#x9f;", "&#X41;", "&#65", "&#65x", "&#0;", "&#x0;", "&#xD800;", "&#xDFFF;",
		"&#x10FFFF;", "&#x110000;", "&#4294967361;", "&#x100000041;", "&#99999999999999999999;", "&#xFFFFFFFFFFFFFFFF;",
		"&#2147483648;", "&#4294967296;", "&#1114112;", "&#128512;", "&#x1F600;", "&#13;", "&#x0D;",
		"a &lt; b &amp;&amp; c &gt; d", "10.0.0.0/8&nbsp;&nbsp;172.16.0.0/12", "<b>&#46;</b>", "&;", "&;;", "& ;",
		"&NotEqualTilde;", "&nvlt;", "&NotEqualTilde", "&fjlig;", "&acE;", "&bne;", "&ThickSpace;", "&nGt;",
		"\u00e9&eacute;\u00fc&uuml;", "&\u00e9;", "&a\u00e9;", "tail&", "tail&#", "tail&#x", "&#xg", "&#9999999999;", "&#xGG;",
	}
	set := entitySet(dir)
	names := make([]string, 0, len(set))
	for name := range set {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		inputs = append(inputs, "&"+name, "&"+name+"Z")
	}
	for n := 0; n <= 0x1FF; n++ {
		inputs = append(inputs, fmt.Sprintf("&#%d;", n), fmt.Sprintf("&#x%X", n))
	}
	var out []pair
	for _, in := range inputs {
		out = append(out, pair{in, html.UnescapeString(in)})
	}
	return out
}

func entitySet(dir string) map[string]bool {
	set := map[string]bool{}
	for _, row := range entities(dir) {
		name := strings.Fields(row)[0]
		set[name] = true
		if !strings.HasSuffix(name, ";") {
			set[name+";"] = true
		}
	}
	return set
}

func quoteCorpus() []pair {
	inputs := []string{
		"", "env", "vng.zone", "a b", "a\"b", "a\\b", "tab\there", "nl\nx", "cr\rx", "\a\b\f\v", "\x00\x01\x1f\x7f",
		"caf\u00e9", "\u0085", "\u00a0", "\u00ad", "\u200b", "\u200e", "\u2028", "\u2029", "\u3000", "\ufeff", "\ufffd",
		"\u0378", "\ue000", "\U0001F600", "\U000E0001", "\U0010FFFF", "\u202e", "\u2066", "\u061c",
		"\u0301", "\u65e5\u672c\u8a9e", "\u043a\u043b\u044e\u0447", "\u1680", "\u180e", "\u2000", "\u200a", "\u202f", "\u205f", "\u2060",
	}
	var out []pair
	for _, in := range inputs {
		out = append(out, pair{in, strconv.Quote(in)})
	}
	return out
}

func main() {
	if len(os.Args) != 3 {
		fmt.Fprintln(os.Stderr, "usage: go run scripts/go-text-tables/main.go <repo-root> <html-source-dir>")
		os.Exit(2)
	}
	root, dir := os.Args[1], os.Args[2]
	version := runtime.Version()
	out := filepath.Join(root, "crates/greennode-sdk/src/go_text")
	fixtures := filepath.Join(root, "crates/greennode-sdk/tests/fixtures/go_text")
	must(os.MkdirAll(fixtures, 0o755))
	head := "# Generated by scripts/go-text-tables/main.go from Go " + version + " (BSD-3-Clause, see licenses/go-LICENSE)."
	write(filepath.Join(out, "entities.txt"), head+"\n# name codepoint [codepoint]", entities(dir))
	write(filepath.Join(out, "replacements.txt"), head+"\n# html replacementTable for 0x80 to 0x9F", replacements(dir))
	write(filepath.Join(out, "printable.txt"), head+"\n# strconv.IsPrint ranges, first last", printable())
	corpora := []struct {
		name   string
		corpus []pair
	}{{"html_unescape.json", htmlCorpus(dir)}, {"quote.json", quoteCorpus()}}
	for _, c := range corpora {
		name, corpus := c.name, c.corpus
		lines := make([]string, len(corpus))
		for i, p := range corpus {
			data, err := json.Marshal(p)
			must(err)
			lines[i] = string(data)
		}
		must(os.WriteFile(filepath.Join(fixtures, name), []byte("[\n"+strings.Join(lines, ",\n")+"\n]\n"), 0o644))
	}
}
