# Get budget cost golden

`get-budget-cost.response.json` is a synthetic wire response. In a disposable
copy of vngcloud `v1.7.0` at commit `8cc8463df39a9c72fa32dcfb359c09618402288f`,
a test decoded the response into `billing.Budget` with Go's `encoding/json` and
passed `billing.GetBudgetOutput` to `renderOutput` in `internal/cli/render.go`.
The golden contains the resulting JSON bytes.
