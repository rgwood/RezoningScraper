# Conditions editorial revision

The earlier eval rewarded precise engineering requirements too generously.
A sidewalk width or fire rating could pass without explaining why that detail
was worth highlighting. The user's review of five posts exposed that gap.

The revised selection and review rules look first for discretionary design and
landscaping demands, open-ended acceptance criteria, limited permit terms, and
requirements made notable by an existing building. Engineering remains eligible.
When there is no strong highlight, the app still produces a neutral summary of
a useful condition. It does not skip the letter or invent controversy.

The factual checks still apply to both kinds of post. Suggested methods must
remain suggestions; they do not make the broader required outcome optional.
Paired design demands must preserve their separate locations. An explicit permit
term must not be confused with a validation deadline, an application-refusal
deadline, or an unspecified expiry. Extension requests and extension decisions
must not acquire each other's deadlines.

## Evaluation changes

The dataset now has 24 cases: 12 frozen City letters and 12 synthetic cases.
The four new regressions cover a routine-only letter, discretionary landscaping
versus standard engineering, a limited permit term versus validation, and an
unspecified expiry that must not borrow the validation period. These cases
informed development; they are not an unseen holdout.

Case notes incorporate the user's feedback. The older Fraser street-works and
Stamp's Landing patio-only calibration examples now fail selection. The old
60/60 result remains historical evidence under the old rubric, not a claim
about this revision. Source-based editorial review happens in this subscription
session; all paid generation uses GLM 5.3 Flash through the official Z.ai
OpenRouter provider.

## Iterations

- The first eight-case pilot generated six summaries and failed to generate two.
  Source review also rejected generated posts: Stamp's Landing omitted the
  one-year term; a synthetic design summary weakened a required outcome to
  advice; a synthetic permit summary moved a request deadline onto approval.
- A six-trial permit-focused check retained the limited terms but still produced
  wording that could shift the start from permit issuance to application approval.
- The first broader run was stopped after it combined a routine validation
  period with an unspecified expiry. Completed outputs are retained; this
  incomplete run is not counted as passing. In-flight charges without saved
  responses cannot be recovered from its traces.
- Prompt-only ranking still let standard legal/engineering conditions displace
  design requests. Candidate categories now make the order explicit, and the
  app selects the highest-priority eligible category before drafting. Model
  classification can still be wrong; local ranking tests do not prove the
  editorial judgment. Routine conditions stay eligible when no stronger angle
  exists, including routine operating/licensing conditions in a sparse letter.
- Repeated rejection could still return to the same passage in every round.
  After two failures on a passage, later rounds prefer a fresh source passage.
  This allows one wording repair before changing angles, while keeping the four-round
  bound and requiring full review/audit of every alternative. If all passages
  have been tried, a corrected draft remains possible. This is a fallback to
  another checked condition, not permission to publish a rejected draft.
- A conservative wording check keeps permit-extension decisions separate from
  request deadlines. It asks for the simpler expiry-plus-written-extension
  wording rather than allowing an added approval-by-expiry claim.
- Source checks now discard an expiry candidate unless its cited passages give
  a specific expiry date or duration. This also applies when the selector puts
  the candidate in the wrong category. A wording check preserves explicit
  building-review context for later permit requirements. Directional claims such
  as “north-facing” need orientation evidence; moving something north does not
  establish which way it faces.
- An audit sometimes uses an ellipsis to join two fragments of a post. The app
  expands these only when both fragments occur verbatim and in order. The same
  evidence and verdict stay attached, and the normal checks still require
  coverage of every factual word. This does not relax source-quotation matching.
- Some responses contain prose followed by a complete JSON object. The parser
  accepts that envelope while retaining the raw response and validating every
  required field. Truncation, multiple objects and trailing prose still fail.
  A formatting failure no longer counts as rejecting the selected condition.
- “One suggested way” explicitly preserves a choice of design methods. The local
  alternatives check now recognizes it, rather than rejecting an accurate post
  because its cited guideline is called “Semi-private or shared open space.”
- Prompt version 4 keeps previously cached summaries separate from this revision.

These are conservative checks, not proof that every summary is correct. The
model still identifies the source passages and interprets most of their meaning.
The saved final source reviews are essential; successful generation alone is not
an editorial pass.

Final results and recorded charges are saved alongside this report. No production
scraper, production database, deployment or real posting is involved in these evals.

## Local verification

`cargo test --locked --all-targets` passes: 110 library, 123 binary and six
integration test executions (the library/binary suites overlap). Two external
integration checks are ignored by default. `cargo clippy --locked --all-targets
-- -D warnings`, formatting and the ten offline Python scorer tests also pass.
The new tests cover editorial priority, routine-only fallback, permit expiry
versus validation, later building review, optional design methods, source retries
and conservative handling of ellipsized claim text.

## Final result

The final complete run (`v11`, prompt version 4) passes 24/24 cases: one fresh
generation per case, mechanical checks, and source-based editorial review by
Codex in the subscription session. No paid grader was used. Reviews are bound
to the exact source, draft, rubric and case notes by fingerprints.

This is a first-pass editorial assessment of known regression cases, not an
independent human review or a reliability guarantee. Twelve trials needed a
repair. A few passing posts retain planning jargon or could be worded more
cleanly; the saved reviews distinguish those from material factual errors.
The previous complete run generated only 22/24 posts, and is retained alongside
the earlier iterations. Passing this run does not erase those failures.

The final run made 151 GLM calls and cost $0.10534 in reported usage. Median
generation time was 38.58 seconds, p95 was 124.86 seconds, and the slowest case
took about 192 seconds. All were within the production summarizer's existing
300-second timeout; this local runner does not itself impose that overall limit.
The recorded subtotal across the iterations is $1.13764. One stopped run had
in-flight requests whose charges are not in the saved traces, so this is not a
reconciled account bill. Every paid call used GLM through official Z.ai.

- [Final posts, evidence and scores](final/scores.json)
- [Source-based review rationales](final/reviews.json)
- [Final source pages and complete model traces](final/raw-run.tar.gz)
- [Every iteration's counts, latency and usage](iteration-history.json)
- [Recorded billing subtotal and its limits](billing.json)

The `iterations/` archives preserve all completed results from superseded runs,
including failures. The interrupted run contains only its completed responses.
