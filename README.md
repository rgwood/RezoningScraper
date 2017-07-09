# RezoningScraper

A small project written for Abundant Housing Vancouver.

Scrapes the City of Vancouver's [rezoning application page](http://rezoning.vancouver.ca/applications/) then notifies people of any changes via Slack. It's an [Azure Function](https://duckduckgo.com/?q=azure+function&t=ffab&ia=web) written in C#, uses HtmlAgilityPack for HTML parsing and Azure Table+Blob storage.
