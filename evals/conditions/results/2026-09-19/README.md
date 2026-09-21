# Conditions summary evaluation

The short summaries need to explain a concrete condition without changing its
scope. A valid 300-character post is not enough: a fluent sentence can turn an
alternative into an extra obligation, a returnable security into a fee, or a
later building-review requirement into a development-permit prerequisite.

This work uses GLM 5.3 Flash through OpenRouter, pinned to the official Z.ai
endpoint with provider fallback disabled. No deployment or production posting
was performed. The existing application-summary model is unchanged.

## Results

The final workflow passed **60/60 source-reviewed trials**: 20 cases, three fresh
generations each. This is my review against the saved source pages, using the
published rubric; it is not independent human validation or an unseen holdout.
The accepted posts are useful, selective summaries, with some still scoring four
rather than five for clarity. Per-post rationales retain those wording caveats.

The final batch made 333 API calls, including repairs and rejected alternatives,
and cost **$0.219705** (about **$0.00366 per summary**). Median latency was 42.98 s;
the 95th percentile was 102.71 s. All 333 responses reported their charge and
identified the provider as Z.AI. The OpenRouter key's cumulative usage rose from
$0 to **$3.030313** across all experiments, including stopped/in-flight calls
not represented in saved traces. See [billing.json](billing.json).

| Run | Source-reviewed passes | Reported API cost |
| --- | ---: | ---: |
| [GLM final (v23)](glm-final/scores.json) | **60/60** | **$0.219705** |
| [Luna](luna/scores.json) | 58/60; two credit-quota failures | Unavailable |
| [GLM pilot v4](glm-pilot-v4/scores.json) | 6/9 | $0.030592 |
| [GLM v5](glm-v5/scores.json) | 57/60 | $0.131560 |
| [GLM pilot v9](glm-pilot-v9/scores.json) | 11/12 | At least $0.055707; one timeout has no reported charge |
| [GLM v11](glm-v11/scores.json) | 57/60 | $0.183309 |
| [GLM pilot v14](glm-pilot-v14/scores.json) | 9/9; expanded run exposed more failures | $0.029713 |
| [GLM v15](glm-v15/scores.json) | 53/60 | $0.163731 |
| [GLM v16](glm-v16/scores.json) | 56/60 | $0.183406 |
| [GLM v17](glm-v17/scores.json) | 55/60 | $0.276147 |
| [GLM v19](glm-v19/scores.json) | 59/60 | At least $0.245933; two calls have no reported charge |
| [GLM v20](glm-v20/scores.json) | 58/60 | $0.297168 |
| [GLM v22](glm-v22/scores.json) | 57/60 | At least $0.265717; three calls have no reported charge |

These runs used evolving workflows, so this is an implementation history, not
a controlled comparison of the models alone. Stopped runs are recorded in
[iteration-history.json](iteration-history.json); missing trials never count as
passes. Raw responses, original page text and errors are preserved in the
archives, including rejected drafts and failed generations.

## Example outputs

These excerpts omit the standard approval prefix and source URL for readability;
the actual complete posts, including their original URLs, fit 300 characters.

- **3631 Point Grey Rd house conversion:** Plans must shave 1 ft off the
  lower-floor ceiling, with slab and roof heights dropping accordingly, to keep
  the massing in check.
- **1410 E 49th Ave daycare conversion:** Building review calls for a 2-hour fire
  separation between the daycare's outdoor play space and the adjacent garbage
  and recycling room.
- **110 E Cordova St hotel conversion:** Existing door swings onto City property
  must be removed or relocated.

## Local validation

The Rust suite passed 171 test executions (two external smoke tests were ignored).
All 10 Python scorer tests passed, along with Clippy, formatting and Python type
checks. Tests cover provider pinning and price caps, failed-draft fallback,
source quotation/qualification checks, cache separation, migrations, and CLI
isolation from posting queues. The network-free scorer refuses a release-gate
pass without matching editorial reviews. No production run was performed.

## What changed

The workflow selects a source-backed condition, writes three concise versions,
reviews them against the full letter, then audits the chosen post against its
cited pages and the first page. That final audit cannot see the previous verdict.
Every stage uses the same configured model. I reviewed evaluation outputs in
the interactive Codex session; there are no paid Astra grader calls.

The earlier runs exposed real problems:

- GLM sometimes omitted JSON fields or added a second object. The official
  endpoint supports JSON mode, not schema-constrained decoding. One local
  format retry now repairs the affected stage without restarting selection or
  giving the writer another stage's schema error.
- Overlong drafts now get measured character counts and a shorter rewrite
  attempt before restarting the whole workflow.
- Generic code levels and numbered bulletin references passed model review.
  Local checks reject that wording. When a code paragraph also names a physical
  demand such as sprinklers, the post must retain that concrete detail.
- The model changed the east half of a named lot into the whole lot. The final
  source audit separately checks scope, quantities and qualifications.
- The final auditor still accepted a deadline broadened to any building permit.
  A deterministic guard rejects that universal permit wording. Model review is
  useful, but it is not a guarantee of correctness.
- A later draft inferred that the entire playground had to be outside an exit
  route from a warning about tripping hazards. The final auditor now quotes the
  source for each substantive claim and checks whether it actually establishes
  that claim. Local checks verify the quotations and claim coverage. This makes
  the judgment inspectable; it does not turn model entailment into a guarantee.
- The editor now returns a complete verdict for its chosen draft, rather than
  repeating the schema for three drafts. All four required gates remain. Local
  checks also reject duplicated project introductions and spelled-out code levels.
- Quoting the source still did not stop the model treating "remove or relocate"
  as supporting "must relocate". A conservative local guard now requires an
  explicit choice when a post retains only one named arm of a quoted OR alternative.
  Generic language covering both options and incidental wording such as
  "necessary or incidental" do not trigger that guard. Ambiguous
  "at permit stage" wording also fails; it repeatedly confused information
  required at application with work already installed then.
- The final audit sometimes supplied a correct exact quotation with the wrong
  page number. The app now corrects it only when it occurs on exactly one of
  the supplied source pages, and records the correction separately from the raw
  response. Qualification checks use the whole source sentence so a clipped
  quotation cannot hide an alternative or a permit's specific purpose.
- Overly literal auditing also rejected valid terminology. Two narrow definitions
  now cite the City's [fire-separation table](https://vancouver.ca/files/cov/vbbl-2025-volume-1-v4-00.pdf#page=131)
  and [public-space access guide](https://vancouver.ca/home-property-development/development-permit-conditions-resource.aspx).
  These permit faithful plain-English translations without inventing a rating,
  duration, unrestricted access or a registration deadline.
- Spatial words matter too: a path **for** an exit stair is not necessarily a
  path **past** it. The writer and final audit now explicitly preserve those
  relationships. A separate regression test fixes a local selection bug that
  mistook occupancy codes beside a concrete separation requirement for an
  opaque code-only lead and discarded the stronger condition.

The normal path makes four calls. Rejected first choices now leave the other
already-written drafts available: each replacement goes through a fresh review
and final audit. There are at most four rounds, with one extra writer attempt
and one format retry per stage: at most 72 calls if every alternative is tried.
All attempts count toward usage and latency. Both review
stages now use high reasoning. The final audit separately lists meaning changes;
any acknowledged change blocks acceptance even when the model calls it minor.

## Reusing the evals

The dataset contains 12 frozen City PDFs and eight synthetic edge cases. The
scorer requires a complete case-by-repeat matrix, valid citations, the full
post plus URL within 300 characters, and matching local editorial reviews.
Accuracy and material qualifications must pass; salience and clarity must each
score at least four out of five. Failed generations stay in the denominator.

The [40 labelled calibration examples](calibration.json) match my local reviews
with no false positives. The examples and reference notes are assistant-curated,
and my reviews are not independent, blinded human validation. All cases have
now informed development and should be treated as regression cases. Add fresh
held-out letters when assessing generalization.

To compare another model, keep the dataset, workflow and rubric fixed, run three
fresh trials per case, and review the source packet using the instructions in
the [evaluation guide](../../README.md). The scorer is network-free and cannot
spend API credits. OpenRouter charges are read from response usage; a missing
charge makes the total unknown instead of being silently treated as zero.
