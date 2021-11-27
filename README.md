# RezoningScraper (Not Currently Operational)

A small project written for [Abundant Housing Vancouver](http://www.abundanthousingvancouver.com).

Scrapes the City of Vancouver's [rezoning application page](http://rezoning.vancouver.ca/applications/) then notifies people of any changes via Slack. It's an [Azure Function](https://duckduckgo.com/?q=azure+function&t=ffab&ia=web) written in C# using .NET Core, uses HtmlAgilityPack for HTML parsing and Azure Table+Blob storage. It costs just a few pennies/month to run.

## Update Nov 2021

The City changed their website and this has been broken since February 2021. 

I'm in the middle of patching it up and modernizing it, things aren't 100 working yet.

## License

Public domain. Do whatever you like with this code, no attribution needed.