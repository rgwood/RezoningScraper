# Conditions summary evals

A short, valid post can still misrepresent a condition or pick the least useful
detail in a ten-page letter. These evals check both mechanics and reporting
quality. They run the same bounded workflow as `--summarize-conditions`, without
touching a database, crawling, posting or sending monitoring events.

## Run a model comparison

Export `OPEN_ROUTER_API_KEY` in your shell. Use a new output directory for every run:

```sh
cargo run --locked --example eval_conditions -- \
  --model open_router::z-ai/glm-5.3-flash --repeats 3 --output evals/conditions/runs/glm

uv run evals/conditions/score.py \
  --results evals/conditions/runs/glm --repeats 3

# Review review-packet.json in the interactive coding session, then supply its local grades:
uv run evals/conditions/score.py \
  --results evals/conditions/runs/glm --repeats 3 --reviews reviews.json
```

To compare another model, change `--model` and the output directory. Keep the
dataset, workflow and rubric fixed. **Scoring makes no API calls.** Editorial
review happens in the interactive coding session (or with a human reviewer),
using `review-packet.json`: source pages, reference notes, all drafts and their
supporting passages. Record the reviewer, per-draft verdicts, source-based
rationales and the supplied input fingerprints in a local reviews JSON file.
Stale or missing reviews fail. The production workflow uses only the
requested generator model, including its internal reviewer. No gold answers or
independent-grader feedback are passed to it.

Start with a small pilot (`--case city-03 --case city-12 --case focused-security`,
one repeat) and inspect both output quality and actual charges before a full run.
Default GLM requests use only the official Z.ai provider (`z-ai/fp8`), with
fallback disabled and price caps ($0.15/M input, $0.50/M output, no
per-request fee). The scorer reports the API's cost, not an estimate from token
counts. Missing cost data leaves the total unknown and exposes only a partial
subtotal. Model/provider identifiers and generation IDs stay in the raw traces.
Overrides must be explicit non-OpenAI OpenRouter models. Direct providers,
OpenAI models, automatic routers and presets are rejected before API calls.
The scorer needs no key. Historical OpenAI results are retained only as records
of earlier experiments; the runner can no longer generate them.

Generation and scoring exit non-zero if any selected trial fails. The first
scoring command still exports the review packet, but exits non-zero until local
editorial reviews are supplied. Explicit `--offline` can pass mechanical checks
alone; its saved release-gate `success` remains false. Calibration
requires at least 95% agreement with the labels, no false positives and no judge
errors. `--calibrate` exports the labelled examples for local review using the
same format; it also makes no API calls. Labels and the rubric are there to make
future reviews consistent, not to replace reading the actual source.

The release gate is **every trial passing** length/citation checks, factual
accuracy, material qualifications, salience and clarity. Run at least three
fresh trials per case before switching models. Do not delete failures, rerun
only the failed cases, or count a cached summary as another trial. The scorer
requires the complete case × repeat matrix and rejects duplicates or mixed
models/workflows. A finite passing set is evidence, not a guarantee for unseen
letters.

For iteration, `--split development` limits both commands to the real letters.
`--case city-05` selects an individual case. Use the same filters and `--repeats`
on generation and scoring. Once a holdout informs a change, treat it as a
regression case; add fresh held-out letters or edge cases for future changes.

## What is checked

- The entire post, including the original URL, fits 300 Unicode code points.
  Text is complete, with no embedded URL or control characters.
- Supporting evidence contains the complete cited pages. This keeps clauses and
  qualifications together when they cross the generator's internal text chunks.
  The independent reviewer checks that those pages actually support the claims;
  citing a real but irrelevant page still fails.
- The post preserves alternatives, scope, optional recommendations, timing and
  duration. A security deposit must not become a fee; plans required before a
  permit must not become construction completed before a permit.
- It selects a concrete, consequential condition. Generic paperwork loses to
  public works, land access, specific redesigns and operating restrictions.
  “Unusual” or “onerous” is not itself a useful or supported claim.
- It reads naturally without unexplained municipal jargon. The grader scores
  salience and clarity from 1–5; both must be at least 4. Accuracy and preservation
  of qualifications are separate mandatory gates.

`cases.json` contains 12 public City letters and eight synthetic edge cases, with
assistant-curated reference posts, acceptable alternative angles, critical
qualifications, and positive/negative calibration examples. References are
guides, not exact-match targets. They were written from source text before the
new workflow's outputs; they are not independent human annotations.

The PDFs are unchanged archived downloads, with source URLs, retrieval dates
and SHA-256 hashes in the manifest. The runner and scorer verify those hashes.
One letter has an administrative footer-only final page; its extraction test
checks that the narrow exception cannot admit scanned or graphical content.
Synthetic cases deliberately separate qualifications across pages and include
an embedded instruction attack. They test particular failure modes and should
not be mistaken for representative samples of all City letters.
Six were held out during initial development. Two later `focused` cases remove
competing angles so the generator must actually summarize the rental duration
or refundable-security clause; merely choosing another correct fact cannot
pass those probes. All are now reusable regression cases.

## Results and reproducibility

Each generation saves original page text, the final post, evidence, every model
response, errors, round count, latency and token usage. Dataset and workflow
hashes identify the inputs and compiled prompts/code. All retries count toward
latency and usage; the workflow allows at most four rounds/72 calls, including
one extra writer attempt per round using measured validation feedback and a
separate final fact audit that cannot see the earlier editor's verdict. Each
final audit supplies exact source quotations for the condition's claims, checked
locally for source occurrence and coverage of the post. Semantic support remains
a model judgment and is checked again during editorial evaluation. Each
audit must also list changes in meaning, including differences it considers minor;
any recorded change blocks acceptance even if its verdict booleans pass. Each
stage gets at most one local JSON-format retry; the usual path is four calls.
When the final audit rejects a draft, the remaining already-written alternatives
go through a fresh full-letter review and final audit before a new round starts.
Rejected alternatives remain in the trace. This reuses the writer's work without
letting an unaudited fallback through.
Opaque code-level leads use the next eligible ranked fact, with that decision
stored separately from the unmodified model response. Failed
generations stay in the denominator.

The scorer saves `scores.json` and, separately, `calibration.json`, including the
review-manifest and rubric hashes. Local review
fingerprints cover the rubric, case, source pages, post and evidence. Changing
any of these invalidates the review. Review packets omit the producer's model
name. There is no paid-grader implementation or default. Only the generation
command makes API calls; all its stages use `--model`, which defaults to GLM 5.3
Flash through OpenRouter.
The official Z.ai endpoint uses JSON mode with an output example; other adapters
use schema-constrained output where supported. Both paths validate required fields
and citations locally. Provider routing and price caps above apply to the default
GLM model; changing models requires checking that model's pricing separately.
These are live model aliases, so record the run date and repeat comparisons
after provider updates. Local run directories are gitignored.

Offline checks need no credentials:

```sh
cargo test --locked --all-targets
uv run evals/conditions/test_score.py
uvx --with-requirements=evals/conditions/score.py ty check \
  --extra-search-path evals/conditions evals/conditions/score.py evals/conditions/test_score.py

cargo run --locked --example eval_conditions -- \
  --extract-only --output evals/conditions/runs/extraction
```

`score.py --offline` checks existing generations mechanically and explicitly
marks the result as **not an editorial evaluation**. The same applies when no
`--reviews` file is supplied. It cannot establish that a model is ready.
Extraction-only results are useful for calibrating the reviewer
against the labelled posts, but do not count as generated summaries.

If an API credit limit interrupts generation, `--resume-quota-failures` retains
completed trials and retries only errors explicitly reporting exhausted credits.
It requires the identical model, dataset and workflow, and embeds each
interrupted attempt in the replacement record. It cannot selectively retry
editorial failures. Report these interruptions separately from quality failures.
