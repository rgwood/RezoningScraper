# GLM migration checks

Ordinary Slack and Bluesky application posts still used OpenAI after the
conditions workflow moved to GLM. They now share the same OpenRouter model
policy: GLM 5.3 Flash, official Z.ai only, no fallback, with price caps.
Direct OpenAI models and OpenRouter OpenAI/automatic-router overrides fail
before requests. Deployment still needs `OPEN_ROUTER_API_KEY`; nothing was
deployed or posted during these checks.

The final run generated 20/20 usable posts from ten saved public applications,
with two fresh trials each. All were within 140 characters (maximum 136).
I read each final post against the saved source description: both trials passed
the factual checks below. This is an editorial review by the interactive coding
assistant, not an independent human evaluation. These fixtures informed the
iterations, so this is a regression check, not an unseen holdout.

| Project | Source facts checked in both final posts |
| --- | --- |
| 100 W 49th Ave / Langara | Five buildings total, maximum six storeys, 1.66 campus FSR; no extra building added for the Meeting House. |
| 2924 Venables St | Two six-storey buildings, 146 social housing units, 2.06 FSR. |
| 5910–5998 Cambie St | Separate 29-storey residential and 15-storey hotel towers; 168 residential and 270 hotel units where included, ten studios, 10.86 FSR. |
| 524–528 Powell St (DP) | Seven storeys, 114 social housing units, 4.38 FSR; ground/second-floor uses were preserved or their floor location omitted. |
| 724 E 56th Ave | Two three-storey townhouse buildings, 12 secured market rental units, 1.40 FSR. |
| 450 W Georgia St | 23-storey office building, public/commercial space at grade, 15.65 FSR. |
| 1325 W 70th Ave | Six storeys, 65 secured market rental units, 2.38 FSR. |
| 1265–1281 Kingsway | Six storeys, 43 secured market rental units, 3.70 FSR; commercial was not narrowed to retail. |
| 7730–7772 Cambie St | Two six-storey buildings, townhouses at grade, 68 strata units, 2.70 FSR. |
| 24 E Broadway / 2520 Ontario St | 12-storey office building with retail space, 7.75 FSR. |

The main weakness of this sample is that it contains only one DP application
and no change-of-use DP. Expand that coverage before treating these results as
a general application-summary quality benchmark.

## What changed during testing

The first request tried disabling reasoning; the endpoint rejected it. Low and
medium reasoning alone produced some short but inaccurate posts: ground/second
floors became “at grade,” commercial became retail, and a building included in a
total was described as additional. Prompt clarifications alone did not reliably
fix the building-count problem.

The final workflow drafts a post, then asks the same GLM model in a separate
request to check it against the original source. It chooses either aggregate
building counts or individual buildings, rather than mixing both. The checker
corrects the draft without adding omitted facts. Both stages aim below the
140-character cap. An overlong checked response gets one rewrite and a fresh
source check; still-overlong, empty or incomplete responses fail without posting.
This bounds each queue attempt to four calls. The final batch used 42 calls.

Earlier attempts are retained in [previews.json](previews.json), including an
18/20 generation run whose two overlong responses were rejected. Its retained
posts also included a counting error. Generation success is not a factual grade;
only the `final-checked` group has the all-pass review described above.

## Cost and evidence

[Metrics](metrics.json) include the final source/input hashes and per-run costs.
Every retained response identifies `z-ai/glm-5.3-flash` and provider `Z.AI`.
The final 20-post batch cost **$0.00361685**. Across all completed previews,
saved response charges total **$0.01247252**. This is a subtotal: two failed
trials lost their partial response traces, so their charges are not included.
The initial rejected request has no response charge record either.

After the billing counter caught up, OpenRouter key usage rose from the prior
conditions-work snapshot of $3.03031348 to $3.04344646: **$0.01313298**, about
1.3 cents for this migration work. The $0.00066046 above saved response charges
accounts for usage not retained in successful-trial traces. These are key-level
counters, not per-trial invoices. No paid OpenAI model or independent API grader
was used.

Local validation: 183 passing Rust test executions (library/binary overlap),
two ignored external tests, ten passing offline scorer tests, clean formatting
and `cargo clippy --locked --all-targets -- -D warnings`. Tests cover provider
pinning, OpenAI override rejection, full-source delivery to the checker,
returning corrected text, usage capture, length retries and incomplete/empty
responses at either stage. The existing conditions workflow's GLM request
settings are unchanged by the shared-provider refactor; its earlier 60/60
quality result belongs to the prior commit, not a new run of this revision.
