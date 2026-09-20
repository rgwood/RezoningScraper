# RezoningScraper

Scrapes the City of Vancouver's website for rezoning and development applications, then notifies people of any changes via Slack and/or Bluesky. It's a simple standalone app using SQLite as a data store, runs on any major OS. Just copy the app (1 file, no dependencies) to a server and run it with a cron job, no further steps needed.

![image](https://github.com/user-attachments/assets/ae0f5020-de0c-4edb-90f1-d691838b76fa)

![image](https://user-images.githubusercontent.com/26268125/143972856-7f01362c-867c-4a0c-90d7-18c1730bd522.png)

## How to use

Download a binary from [the releases page](https://github.com/rgwood/RezoningScraper/releases) or build it from source ([install Rust](https://rustup.rs/) then run `cargo build --release`).

Run it; on the first launch it will download all ShapeYourCity projects without posting any. On subsequent launches, it will post to Slack and/or Bluesky if credentials are set via argument or environment variable.

Bluesky functionality uses OpenAI for summarizing projects; you will also need to specify an `OPENAI_API_KEY` environment variable.

```

❯ ./rezoning-scraper --help
Usage: rezoning-scraper [OPTIONS]

Options:
      --slack-webhook-url <SLACK_WEBHOOK_URL>
          A Slack Incoming Webhook URL. If specified, will post info about new+modified rezonings to this address. [env: SLACK_WEBHOOK_URL=]
      --bluesky-user <BLUESKY_USER>
          Bluesky username. Required for posting to Bluesky [env: BLUESKY_USER=]
      --bluesky-password <BLUESKY_PASSWORD>
          Bluesky password. Required for posting to Bluesky [env: BLUESKY_PASSWORD=]
      --api-cache
          Use cached API responses (up to 1 hour old) when available
      --skip-update-db
          Skip updating the local database (useful for testing)
      --monitoring-test <MONITORING_TEST>
          Send a test status to monitoring without running the scraper [possible values: ok, critical]
  -h, --help
          Print help
  -V, --version
          Print version
```

## Approval tracking

Approval notices and their conditions can appear months after the original
application. Approval tracking keeps those notices and copies of the linked PDFs
in SQLite, independently of the posting queues. It is opt-in.

To collect approvals without summarizing or posting anything:

```console
rezoning-scraper --tracking-only --database approvals.db
```

`--tracking-only` bypasses all LLM, Slack, Bluesky, and monitoring work, even if
credentials are configured or messages are already queued. It does not update
the ordinary `Projects` snapshots, so it won't consume new-project notifications
from a later normal run. The separate database above keeps testing isolated.

`--track-approvals` adds collection to a normal run, which still processes its
existing posting queues. Neither option posts approval updates. Both conflict
with `--skip-update-db`. Collection failures make the run fail before entering
the posting pipeline; successfully collected evidence is kept for the next run.

The tracker distinguishes rezoning approval, development approval, development
approval subject to conditions, and permit issuance. It reads explicit notices
from the API's description and archival message. An archived consultation alone
does not count as approval, and a development application's reference to an
earlier rezoning approval does not count as development approval.

For each notice it saves the project name and URL, application number when
available, decision type, authority, stated date, notice text, and first/last
observation times. `IsBaseline = 1` means the notice was present on that project's
first tracking scan, not that the approval just happened. Unknown decision dates
are stored as an empty string; observation timestamps are Unix seconds in UTC.
Reworded notices are retained as separate observations, not necessarily separate
decisions. Missing notices never delete earlier evidence.

For projects with a recognized decision, it checks the project description and
document library for links labelled “Prior-to letter” or “Conditions of approval”.
It follows Shape Your City's download pages and stores distinct PDF contents as
SQLite BLOBs, keeping older versions when a URL's contents change. Repeated bytes
do not create another copy. Downloads are limited to 20 MiB each; failed downloads
retain the link and error and are retried on a later run. The database will grow
as documents accumulate.

Unchanged projects with known decisions are checked for documents again after
24 hours, including projects whose documents were initially missing. Changed
notice/description text triggers an earlier check. These checks happen when you
run the scraper; the option does not install a schedule.

This is a conservative first pass. Unfamiliar approval wording may be missed,
and generic council-report links are not treated as conditions documents. It
archives PDFs; the separate summary command below analyzes them. It cannot
reconstruct conditional-approval dates or documents removed before tracking began.

Inspect the history and linked documents with SQLite:

```sql
SELECT ProjectName, ApplicationNumber, Kind, DecisionDate,
       datetime(FirstSeen, 'unixepoch') AS FirstObservedUTC,
       IsBaseline, Notice
FROM ApprovalEvents
ORDER BY FirstSeen DESC, Id DESC;

SELECT d.ProjectId, d.Title, d.SourceUrl, d.LastError,
       v.Id AS VersionId, length(v.Content) AS PdfBytes
FROM ApprovalDocuments d
LEFT JOIN ApprovalDocumentVersions v ON v.DocumentId = d.Id
ORDER BY d.Id, v.Id;
```

In the SQLite CLI, export a version using its `VersionId`:

```sql
SELECT writefile('conditions.pdf', Content)
FROM ApprovalDocumentVersions WHERE Id = 1;
```

For a limited test, `--tracking-only --projects-file response.json --database test.db`
reads a saved projects API response instead of querying the API. Document pages
and PDFs are still fetched over HTTP. Automated tests use saved page excerpts,
in-memory databases, localhost fixture servers, and isolated CLI databases:

```console
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

## Conditions summaries

Conditions letters can be long, and approval does not mean the applicant can
start building. The summary command drafts a short post from the PDFs
already collected by approval tracking:

```console
rezoning-scraper --summarize-conditions --database approvals.db --summary-limit 3
```

This uses `OPENAI_API_KEY` and the same `gpt-5.6-luna` model as application
summaries. It prints one short paragraph and the source URL, and saves structured JSON in
SQLite. It does not crawl, post, consume posting queues, or send monitoring
events. Run approval tracking separately to collect new documents.

The SDK is `genai` 0.6.5, which routes this model through OpenAI's Responses API.
Conditions requests use low reasoning effort, a 5,000-token output limit and a 90-second timeout;
incomplete responses are rejected.

The post identifies the project and highlights one or two concrete conditions,
with a bias toward potentially burdensome or distinctive demands: off-site work,
utility upgrades, land rights, payments, redesigns, and specialist studies.
It states the requirements rather than making unsupported claims that they are
unusual or unnecessary. Routine requirements rank lower when more consequential
conditions are available.
It omits the checklist, page references, and routine administrative steps.
The prompt aims for 160–180 characters of prose. The hard limit applies to
the entire draft: the full source URL, separator, and prose must fit within
300 characters. Length is validated after generation rather than constrained by
the JSON schema, which produced cut-off sentences in live testing. Longer
responses are rejected, never cut off. Control characters and drafts without a
closing period are also rejected. Counting Unicode
scalar values is conservative for Bluesky's grapheme limit.

Supporting requirements, PDF page references, and limitations stay in SQLite.
The prompt distinguishes conditions before permit
issuance from permit terms, occupancy requirements, and advisory comments. It
preserves alternatives and qualifications, highlights explicit fees and
deadlines, and avoids guessing costs or describing requirements as onerous.

Each supporting requirement has a quote in the saved JSON. The app checks
that the quote occurs on the cited page before saving the summary. This catches
invented citations, but does not prove the paraphrase is correct. These are
AI-generated, selective summaries, not complete compliance checklists.
The last raw model response is retained in `RawResponse` for reviewing rejected
citations or overlong drafts; only validated responses populate `SummaryJson`.
Prompt version 2 generates compact posts; earlier detailed summaries remain
stored under their original version.

PDF text extraction is built into the binary; no external PDF tools are needed.
The app keeps page boundaries and saves the text used for the summary. It rejects
malformed PDFs, pages with very little readable text, documents over 100 pages,
and extracted text over 120,000 bytes rather than silently truncating the letter.
There is no OCR yet, so scanned documents may need manual review.

By default a run processes up to 10 PDFs, newest archived versions first. Set
`--summary-limit` from 1 to 100 to change that. Completed summaries are cached by
PDF version, model, prompt version, and extractor version. New PDF versions get
their own summaries; older summaries remain available. No model call is made
when displaying a cached result:

```console
rezoning-scraper --summarize-conditions --database approvals.db --document-version 1
```

Failures are saved and retried on later runs, up to three attempts. One failed
document does not stop the rest of the batch, but the command exits non-zero if
any document in that batch failed. Use `--retry-failed-summaries` to explicitly
retry documents that have exhausted their attempts. Missing API credentials fail
before consuming any attempts. Cached summaries can be viewed without a key.

Inspect results and failures with SQLite:

```sql
SELECT DocumentVersionId, Model, PromptVersion,
       json_extract(SummaryJson, '$.overview') AS Overview,
       Attempts, LastError
FROM ConditionsSummaries
ORDER BY DocumentVersionId DESC;

SELECT s.DocumentVersionId,
       json_extract(r.value, '$.requirement') AS Requirement,
       json_extract(r.value, '$.page') AS Page,
       json_extract(r.value, '$.evidence') AS Evidence
FROM ConditionsSummaries s, json_each(s.SummaryJson, '$.requirements') r;
```

Tests include a real six-page childcare conditions letter, quote/page validation,
retry limits, PDF and prompt version changes, a local mock OpenAI server, and CLI
checks that existing posting queues remain untouched. The default test suite
never uses a real API key. An opt-in smoke test checks the existing application
summarizer against the live API without posting:

```console
cargo test --lib live_application_summary -- --ignored --nocapture
```

## Monitoring

The scraper reports run status and queue depth to a local Datadog Agent. It
also exits non-zero and writes a structured log when anything fails.

```console
rezoning-scraper --monitoring-test critical
rezoning-scraper --monitoring-test ok
```

## License

Public domain. Do whatever you like with this code, no attribution needed.
