# Full-history graph benchmark

From the workspace root:

```sh
GITUI_BENCH_REPO=/absolute/path/to/repository \
  cargo bench -p asyncgit --bench full_history
```

The repository must have a readable `HEAD`. The benchmark exercises reading all
commits reachable from `HEAD`.

## Comparison

- `full_history/without_graph`: fetch the entire history through `AsyncLog`,
  extract its commit IDs, and visit consecutive 1,200-commit windows.
- `full_history/with_graph`: do the same, additionally calling the public
  `AsyncLog::get_graph_rows` for every window. This gives us a good idea of the
  overhead added by creating the visual commit graph.

Fetching finishes before graph generation begins. The difference between both
variants can thus be attributed to graph creation.

## Timing boundaries

The timed region includes:

- Starting and waiting for the complete async history fetch.
- Extracting commit IDs.
- In `with_graph`, creation and disposal of each window's expanded rows.
- A couple of assertions to verify the benchmark iterates over the expected
  number of commits.

The timeout defaults to 1,200 seconds per iteration. Set
`GITUI_BENCH_TIMEOUT_SECS` to change it. It bounds notification waits and is
checked between windows; it cannot interrupt a synchronous Git operation or
cancel an already-running `AsyncLog` task.

## Smoke test

Execute each case once without statistical sampling:

```sh
GITUI_BENCH_REPO=/absolute/path/to/repository \
  cargo bench -p asyncgit --bench full_history -- --test
```

The smoke run executes the benchmark's runtime assertions against the selected
repository. There is no separate generated-repository test suite.

## JSON output

A normal benchmark run writes Criterion's JSON results under:

```text
target/criterion/full_history/without_graph/new/estimates.json
target/criterion/full_history/with_graph/new/estimates.json
```

For example, print the mean with-graph time in milliseconds:

```sh
jq '.mean.point_estimate / 1000000' \
  target/criterion/full_history/with_graph/new/estimates.json
```
