using RezoningScraper;
using Spectre.Console;
using System.Diagnostics;
using System.Reflection;
using static Spectre.Console.AnsiConsole;

MarkupLine($"[green]Welcome to RezoningScraper v{ Assembly.GetExecutingAssembly().GetName().Version}[/]");
WriteLine();

await Status().StartAsync("Opening DB...", async ctx => 
{
    try
    {
        var db = DbHelper.CreateOrOpenFileDb("RezoningScraper.db");
        db.InitializeSchemaIfNeeded();

        ctx.Status = "Loading token...";

        var tokenFromDb = db.GetToken();
        string jwt;

        if (tokenFromDb != null && tokenFromDb.Expiration > DateTimeOffset.UtcNow.AddMinutes(1))
        {
            jwt = tokenFromDb.JWT;
            WriteLine($"Loaded API token from database. Cached token will expire on {tokenFromDb.Expiration}");
        }
        else
        {
            WriteLine("Getting latest anonymous user token from shapeyourcity.ca");
            var client = new HttpClient() { Timeout = TimeSpan.FromSeconds(20) };
            var htmlToParse = await client.GetStringAsync("https://shapeyourcity.ca/embeds/projectfinder");

            jwt = TokenHelper.ExtractTokenFromHtml(htmlToParse);

            var expiration = TokenHelper.GetExpirationFromEncodedJWT(jwt);

            WriteLine($"Retrieved JWT with expiration date {expiration}");

            db.SetToken(new Token(expiration, jwt));
            WriteLine($"Cached JWT in local database");
        }

        ctx.Status = "Querying API...";
        WriteLine("Starting API query...");
        var stopwatch = Stopwatch.StartNew();
        var results = await API.GetAllProjects(jwt).ToListAsync();
        MarkupLine($"API query finished: retrieved {results.Count} projects in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");

        ctx.Status = "Comparing against existing database...";

        List<Datum> newProjects = new();
        List<(Datum Old, Datum Latest)> modifiedProjects = new();

        stopwatch.Restart();
        var tran = db.BeginTransaction();
        foreach (var project in results)
        {
            if (db.ContainsProject(project.id!))
            {
                var oldVersion = db.GetProject(project.id!);
                if (ProjectsHaveChanged(oldVersion, project))
                {
                    modifiedProjects.Add((oldVersion, project));
                }
            }
            else
            {
                newProjects.Add(project);
            }

            db.UpsertProject(project);
        }
        tran.Commit();

        MarkupLine($"Upserted {results.Count} projects to the DB in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");
        MarkupLine($"Found [green]{newProjects.Count}[/] new projects and [green]{modifiedProjects.Count}[/] modified projects.");

        // We've got the info we need; now do something with it
        HandleNewProjects(newProjects);
        HandleModifiedProjects(modifiedProjects);
    }
    catch (Exception ex)
    {
        MarkupLine("[red]Fatal exception thrown[/]");
        WriteException(ex);
    }
});

void HandleNewProjects(List<Datum> newProjects)
{
    WriteLine();
    MarkupLine("[bold underline green]New Projects[/]");
    WriteLine();

    foreach (var project in newProjects)
    {
        MarkupLine($"[bold underline]{project.attributes!.name!.EscapeMarkup()}[/]");
        MarkupLine($"State: {project.attributes.state.EscapeMarkup()}");

        var tags = project?.attributes?.projecttaglist ?? new string[0];
        if (tags.Any())
        {
            MarkupLine($"Tags: {string.Join(',', tags).EscapeMarkup()}");
        }

        WriteLine($"URL: {project!.links!.self}");

        WriteLine();
    }
}

void HandleModifiedProjects(List<(Datum Old, Datum Latest)> modifiedProjects)
{
    // TODO: implement
}

//WriteLine("Press any key to exit");
//System.Console.ReadKey();

bool ProjectsHaveChanged(Datum oldVersion, Datum newVersion)
{
    // todo: check more fields (all attributes), and return data about what exactly changed

    return oldVersion?.attributes?.name != newVersion?.attributes?.name;
}