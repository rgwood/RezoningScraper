# RezoningScraper

A small project written for [Abundant Housing Vancouver](http://www.abundanthousingvancouver.com).

Scrapes the City of Vancouver's [rezoning application page](http://rezoning.vancouver.ca/applications/) then notifies people of any changes via Slack. It's an [Azure Function](https://duckduckgo.com/?q=azure+function&t=ffab&ia=web) written in C# using .NET Core, uses HtmlAgilityPack for HTML parsing and Azure Table+Blob storage.

#### Build
Just run `dotnet build`.

#### Development + Deployment

I use the Azure Functions extension in VS Code. 

To run this locally, you may first need to use the "Initialize project for use with VS Code" command.

To deploy, use the "Deploy to function app" command and point it at the publish output folder (`/RezoningScraper.Functions/bin/Release/netstandard2.0/publish`).

Troubleshooting: try opening the `RezoningScraper.Functions` folder and deploying from there – sometimes doesn't work from the solution directory, need to figure out why.

#### Tests
`dotnet test`

Note that there is a bug (?) in the current version of .NET Core that means when you run `dotnet test` on a solution, it attempts to run tests in _all_ projects (and fails on projects that contain no tests). I'm using [this handy fix by Martin Ullrich ](https://dasmulli.blog/2018/01/20/make-dotnet-test-work-on-solution-files/), as found in [this GitHub issue](https://github.com/Microsoft/vstest/issues/1129).