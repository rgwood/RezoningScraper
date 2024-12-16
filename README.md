# RezoningScraper

A small project written for [Abundant Housing Vancouver](http://www.abundanthousingvancouver.com).

Scrapes the City of Vancouver's website for rezoning and development applications, then notifies people of any changes via Slack. It's a simple standalone app using SQLite as a data store, runs on any major OS. Just copy the app (1 file, no dependencies) to a server and run it with a cron job, no further steps needed.

![image](https://user-images.githubusercontent.com/26268125/143966385-3ff0f2ae-b8ef-4bf1-bc17-c52aa7ed7e16.png)

![image](https://user-images.githubusercontent.com/26268125/143972856-7f01362c-867c-4a0c-90d7-18c1730bd522.png)

(screenshot is from a slightly older version that was written in C#)

## How to use

Download a binary from [the releases page](https://github.com/rgwood/RezoningScraper/releases) or build it from source ([install Rust](https://rustup.rs/) then run `cargo build --release`). Then run it:

```

❯ ./rezoning-scraper --help
Usage: rezoning-scraper [OPTIONS]

Options:
      --slack-webhook-url <SLACK_WEBHOOK_URL>
          A Slack Incoming Webhook URL. If specified, will post info about new+modified rezonings to this address.
      --api-cache
          Use cached API responses (up to 1 hour old) when available
      --skip-update-db
          Skip updating the local database (useful for testing)
  -h, --help
          Print help
  -V, --version
          Print version
```



## License

Public domain. Do whatever you like with this code, no attribution needed.

## To Do

- [ ] Figure out better error handling. Ideally we'd put messages on a queue to be processed.
  - [ ] Have multiple queues; one for Slack things, one for Bluesky
  - [ ] Would be cool to make this a library. Give it a SQLite DB and let it go wild. But maybe start small and pull it into a library later
  - [ ] Need dead letter queues too
  - [ ] Backed by a MessageQueue table and a DeadLetterQueue table
  - [ ] What should API look like? I think give it a table name and make it generic over the types of messages expected
    - [ ] dequeue<T>(table_name: string) -> Result<Option<T>>
      - or similar (may want to return a Message<T> that provides extra data)
- [ ] Hook up to Sentry for error reporting
- [ ] Add option to post to Bluesky