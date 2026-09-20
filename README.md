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
archives PDFs without extracting or summarizing their contents. It cannot
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

## Monitoring

The scraper reports run status and queue depth to a local Datadog Agent. It
also exits non-zero and writes a structured log when anything fails.

```console
rezoning-scraper --monitoring-test critical
rezoning-scraper --monitoring-test ok
```

## License

Public domain. Do whatever you like with this code, no attribution needed.
