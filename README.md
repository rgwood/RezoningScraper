# RezoningScraper

Scrapes the City of Vancouver's website for rezoning and development applications, then notifies people of any changes via Slack and/or Bluesky. It's a simple standalone app using SQLite as a data store, runs on any major OS. Just copy the app (1 file, no dependencies) to a server and run it with a cron job, no further steps needed.

![image](https://github.com/user-attachments/assets/ae0f5020-de0c-4edb-90f1-d691838b76fa)

![image](https://user-images.githubusercontent.com/26268125/143972856-7f01362c-867c-4a0c-90d7-18c1730bd522.png)

## How to use

Download a binary from [the releases page](https://github.com/rgwood/RezoningScraper/releases) or build it from source ([install Rust](https://rustup.rs/) then run `cargo build --release`).

Run it; on the first launch it will download all ShapeYourCity projects without posting any. On subsequent launches, it will post to Slack and/or Bluesky if credentials are set via argument or environment variable.

Bluesky functionality uses Claude for summarizing projects; you will also need to specify an ANTHROPIC_API_KEY via environment variable.

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

## Monitoring

The scraper reports each run to a local Datadog Agent through DogStatsD. It
sends:

- the `rezoning_scraper.run` service check
- `rezoning_scraper.queue.depth`, tagged by queue
- `rezoning_scraper.dead_letter.depth`, tagged by queue
- structured error logs with `service:rezoning-scraper` and `env:production`

Set `DD_SERVICE`, `DD_ENV`, or `DOGSTATSD_ADDRESS` to override those defaults.

Create a Datadog service check monitor for `rezoning_scraper.run`. Alert on one
critical check and on 20 hours without data, then add an email address to the
notification message. The no-data alert catches a timer or host failure as well
as errors reported by the scraper.

Test the whole path without scraping or posting anything:

```console
rezoning-scraper --monitoring-test critical
rezoning-scraper --monitoring-test ok
```

Wait for the critical notification before sending the OK status.

## License

Public domain. Do whatever you like with this code, no attribution needed.
