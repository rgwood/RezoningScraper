# Application summary checks

Slack and Bluesky use the same `project_to_tweet` function. It now uses GLM 5.3
Flash through OpenRouter's official Z.ai endpoint, sharing the model policy and
price caps with conditions summaries. No OpenAI route or model fallback is used.

See the [migration results](results/2026-09-19/README.md) for saved previews,
source review and measured charges, including earlier unsuccessful iterations.

Run a local preview against the saved public projects fixture:

```sh
cargo run --locked --example eval_applications -- \
  --limit 10 --repeats 2 --output evals/conditions/runs/applications
```

Set `OPEN_ROUTER_API_KEY` first. The command never opens a database or posts. It
saves the source description, generated text, errors and latency. Completed trials
also retain response usage, including draft/source-check stages and rejected
overlong text. A failed trial currently retains the error without its partial
responses, so its cost is missing from the response subtotal; reconcile total
spend with the OpenRouter key usage when errors occur.
The source checker uses the same GLM model, not a paid independent grader.
It refuses to overwrite
trials. `--project-id ID` can be repeated to select specific projects; `--projects`
accepts another saved Shape Your City response.

Generation success means nonempty, completed text within 140 characters. Read
the actual saved descriptions to check address, height, unit count, tenure,
proposed use and density. An omitted secondary fact is fine; invented totals or
claims of approval are not. Preserve use labels (commercial is not necessarily
retail), floor locations and totals that already include listed components.
This small migration check is separate from the
conditions-summary regression suite.

Local tests verify both rezoning and DP requests use the same pinned provider,
send the original source and draft to a separate check, return its corrected text,
preserve usage metadata, reject incomplete/empty text at either stage, and rewrite
an overlong checked post once without truncating it. The shared policy rejects direct OpenAI and
OpenRouter's OpenAI/automatic-router paths, including CLI overrides. Existing
queue/CLI isolation tests ensure tracking and conditions commands cannot post.
