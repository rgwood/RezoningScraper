# RezoningScraper

A small project written for [Abundant Housing Vancouver](http://www.abundanthousingvancouver.com).

Scrapes the City of Vancouver's website for rezoning and development applications, then notifies people of any changes via Slack. It's a simple standalone console app using  SQLite as a data store. Just copy the app (1 file, no dependencies) to a server and run it with a cron job, no further steps needed.

![image](https://user-images.githubusercontent.com/26268125/143966385-3ff0f2ae-b8ef-4bf1-bc17-c52aa7ed7e16.png)

![image](https://user-images.githubusercontent.com/26268125/143966785-750be3e1-1a91-4fe2-8fdd-d6e929eeeae0.png)


## How to use

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
