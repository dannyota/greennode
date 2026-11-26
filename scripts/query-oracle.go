package main

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/jmespath/go-jmespath"
)

type Case struct {
	Expression string `json:"expression"`
	Input      any    `json:"input"`
	Stage      string `json:"stage"`
	Result     any    `json:"result,omitempty"`
	Excluded   string `json:"excluded,omitempty"`
}

func run(c *Case) {
	defer func() {
		if recover() != nil {
			c.Stage = "panic"
		}
	}()
	query, err := jmespath.Compile(c.Expression)
	if err != nil {
		c.Stage = "compile"
		return
	}
	result, err := query.Search(c.Input)
	if err != nil {
		c.Stage = "evaluation"
		return
	}
	c.Stage, c.Result = "success", result
}

func main() {
	var input any
	if err := json.Unmarshal([]byte(`{"items":[{"name":"b","n":2},{"name":"a","n":1},{"n":3}],"numbers":[3,1,2],"text":"héllo","obj":{"a":1}}`), &input); err != nil {
		panic(err)
	}
	expressions := []string{
		"abs(`-2`)", "avg(numbers)", "ceil(`1.2`)", "contains(text, 'é')", "ends_with(text, 'lo')", "floor(`1.8`)", "join('-', items[].name)", "keys(obj)", "length(text)", "map(&name, items)", "max(numbers)", "max_by(items, &n).n", "merge(obj, `{\"b\":2}`)", "min(numbers)", "min_by(items, &n).n", "not_null(`null`, text)", "reverse(text)", "sort(numbers)", "sort_by(items, &n)[].n", "starts_with(text, 'hé')", "sum(numbers)", "to_array(text)", "to_number('true')", "to_string(obj)", "type(numbers)", "values(obj)",
		"items[].name", "items[?n > `1`].n", "numbers[::-1]", "numbers[0:3:2]", "numbers[-1]", "`1` == `1.0`", "`1` == `1.000000000000001`", "`9007199254740992` == `9007199254740993`", "'a' < 'b'", "[text numbers]", "unknown(@)", "numbers[::0]", "contains(`[[]]`, `[]`)", "[1", "items[].{x: name}", "'raw\\'quote'", "items | [0].name", "[&name]",
	}
	expressions = append(expressions, "[sort_by(numbers, &@), numbers]", "map(&length(@), `[null, []]`)", "to_number('0."+strings.Repeat("1", 801)+"')", "'"+strings.Repeat("x", 16385)+"'")
	cases := make([]Case, 0, len(expressions))
	for _, expression := range expressions {
		encoded, err := json.Marshal(input)
		if err != nil {
			panic(err)
		}
		var copy any
		if err := json.Unmarshal(encoded, &copy); err != nil {
			panic(err)
		}
		c := Case{Expression: expression, Input: copy}
		run(&c)
		if err := json.Unmarshal(encoded, &c.Input); err != nil {
			panic(err)
		}
		if expression == "[sort_by(numbers, &@), numbers]" {
			c.Excluded = "Go sort_by mutates its input; Rust sorts a copy"
		}
		if expression == "map(&length(@), `[null, []]`)" {
			c.Excluded = "Go stops on the first ordinary error; Rust continues to detect later safety errors"
		}
		if strings.HasPrefix(expression, "to_number('0.") {
			c.Excluded = "Go truncates decimals past 800 significant digits"
		}
		if len(expression) > 16384 {
			c.Excluded = "Rust rejects expressions longer than 16 KiB"
		}
		if expression == "[&name]" {
			c.Excluded = "Expression references outside direct function arguments are evaluation errors"
		}
		if expression == "[1" {
			c.Excluded = "Rust rejects unterminated indices at compile time"
		}
		if expression == "contains(`[[]]`, `[]`)" {
			c.Excluded = "Go panics on uncomparable values; Rust returns an evaluation error"
		}
		cases = append(cases, c)
	}
	data, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		panic(err)
	}
	if _, err := fmt.Println(string(data)); err != nil {
		panic(err)
	}
}
