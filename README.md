# RezoningScraper

A small project written for [Abundant Housing Vancouver](http://www.abundanthousingvancouver.com).

Scrapes the City of Vancouver's website for rezoning and development applications, then notifies people of any changes via Slack. It's a simple standalone app using SQLite as a data store, runs on any major OS. Just copy the app (1 file, no dependencies) to a server and run it with a cron job, no further steps needed.

![image](https://user-images.githubusercontent.com/26268125/143966385-3ff0f2ae-b8ef-4bf1-bc17-c52aa7ed7e16.png)

![image](https://user-images.githubusercontent.com/26268125/143972856-7f01362c-867c-4a0c-90d7-18c1730bd522.png)

## How to use

Download from [the releases page](https://github.com/rgwood/RezoningScraper/releases) or build it from source (requires the .NET 6 SDK). Then run it:

```
❯ .\RezoningScraper.exe --help
RezoningScraper
  A tool to detect new+modified postings on Vancouver's shapeyourcity.ca website. Data is stored in a local SQLite database next to the executable.

Usage:
  RezoningScraper [options]

Options:
  --slack-webhook-url <slack-webhook-url>  A Slack Incoming Webhook URL. If specified, RezoningScraper will post info about new+modified rezonings to this address.
  --version                                Show version information
  -?, -h, --help                           Show help and usage information
```

## License

Public domain. Do whatever you like with this code, no attribution needed.

## To Do

- [ ] GitHub Actions for build+test
- [ ] GitHub Action to automatically publish trimmed executables as a release
- [ ] `--dry-run` flag that doesn't update the local DB
- [ ] Add retries to the token fetching. It times out fairly often
- [ ] Some kind of development cache thingy. The API is slowwwww and that makes iterative development painful
- [ ] Discord integration?
- [ ] Twitter integration?
- [ ] Display more project fields?
- [ ] [This is rough](https://github.com/rgwood/RezoningScraper/blob/ca38460e6ffbd177ef842b0362ff3449737bf3a5/RezoningScraper/TokenHelper.cs#L54-L60), there's gotta be a better way to query JSON
- [ ] Consider archiving old versions of projects
- [ ] Escape `|` character in project names when posting to Slack - it breaks their link format. Ex:  "Chinatown Memorial Square Redesign |
唐人街紀念廣場重新設計"
- [ ] Consider excluding `published -> archived` state transitions from Slack. Lots of noise, not particularly useful
