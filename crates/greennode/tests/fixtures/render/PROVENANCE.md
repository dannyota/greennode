# Float fixture provenance

`floats.tsv` was generated using Go 1.27.4 with random seed `20261125`. The
throwaway generator lived outside the repository in the session scratchpad. The
fixture contains finite random float64 and float32 bit patterns, together with
signed zero, extrema, notation boundaries, decimal powers, and adjacent
representable values. Its columns are the source-width and hexadecimal bits, Go
JSON, fixed-point text after f64 normalization, and Go map-key text.
