# RezoningScraper (Not Currently Operational)

A small project written for [Abundant Housing Vancouver](http://www.abundanthousingvancouver.com).

Scrapes the City of Vancouver's [rezoning application page](http://rezoning.vancouver.ca/applications/) then notifies people of any changes via Slack. It's an [Azure Function](https://duckduckgo.com/?q=azure+function&t=ffab&ia=web) written in C# using .NET Core, uses HtmlAgilityPack for HTML parsing and Azure Table+Blob storage. It costs just a few pennies/month to run.

## Update Nov 2021

The City changed their website and this has been broken since February 2021. 

I'm in the middle of patching it up and modernizing it, things aren't 100% working yet.

Previously this relied on cloud compute+storage. I've soured on the cloud somewhat in recent years, my plan for version 2 is to make this a simple standalone console app with SQLite as a data store. Just copy the app (1 file) to a server and run it with a cron job, no further steps needed.

## License

Public domain. Do whatever you like with this code, no attribution needed.
