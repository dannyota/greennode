package main

import (
	"fmt"
	"unicode"
	"unicode/utf8"
)

type pair struct {
	source rune
	offset rune
}

func main() {
	fmt.Println("pub(super) const RANGES: &[(u32, u32, u32, i32)] = &[")
	var pairs []pair
	for r := rune(0); r <= utf8.MaxRune; r++ {
		canonical := r
		for next := unicode.SimpleFold(r); next != r; next = unicode.SimpleFold(next) {
			canonical = min(canonical, next)
		}
		if canonical != r {
			pairs = append(pairs, pair{r, canonical - r})
		}
	}
	for len(pairs) != 0 {
		first := pairs[0]
		count, step := 1, rune(1)
		if len(pairs) > 1 && pairs[1].offset == first.offset {
			step = pairs[1].source - first.source
			for count < len(pairs) && pairs[count].offset == first.offset && pairs[count].source == first.source+rune(count)*step {
				count++
			}
		}
		fmt.Printf("    (0x%X, 0x%X, %d, %d),\n", first.source, pairs[count-1].source, step, first.offset)
		pairs = pairs[count:]
	}
	fmt.Println("];")
}
