# Documentation

Each fact lives in the document type that owns it. Link to that source instead
of copying it.

| Question                                      | Authority                                                                      |
| --------------------------------------------- | ------------------------------------------------------------------------------ |
| What should the product become?               | [`design/`](design/README.md)                                                  |
| Why was a choice made?                        | [`adr/`](adr/README.md)                                                        |
| What did the Go CLI do?                       | [vngcloud](https://github.com/dannyota/vngcloud) at tag `v1.0.1`, the baseline |
| What does the repository implement now?       | Code and tests                                                                 |
| What runs next?                               | [`plans/`](plans/)                                                             |
| How should documents and comments be written? | [`standards/engineering.md`](standards/engineering.md)                         |

An accepted ADR wins over design text. A greennode design wins over the baseline
and over a plan. The baseline decides any behavior no greennode design covers.

| Path         | Contents                          | Lifecycle                              |
| ------------ | --------------------------------- | -------------------------------------- |
| `design/`    | Intended product and architecture | Kept current; amended by accepted ADRs |
| `adr/`       | One decision per topic            | Updated with owner approval            |
| `plans/`     | Remaining work and its order      | A plan is deleted when its work lands  |
| `standards/` | Writing and comment rules         | Living repository policy               |
