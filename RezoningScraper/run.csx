#r "System.Collections"
#r "System.IO"

using HtmlAgilityPack;
using System;
using System.Configuration;
using System.Collections.Generic;
using System.Data;
using System.Data.SqlClient;
using System.Linq;
using System.Net;
using System.Text;
using System.Text.RegularExpressions;
using Dapper;
using Dapper.Contrib.Extensions;
using System.Net.Http;

private class Rezoning
{
    public int Id { get; set; }
    public string Name { get; set; }
    public string Status { get; set; }
    public string Info { get; set; }
}

public static void Run(TimerInfo myTimer, TraceWriter log)
{
    log.Info($"C# Timer trigger function executed at: {DateTime.Now}");
    var rezonings = GetRezoningsFromDb();
    
    var pageUri = new System.Uri(ConfigurationManager.AppSettings["RezoningPageUri"]);
    var web = new HtmlWeb();
    var doc = web.Load(pageUri.ToString());
    var desc = doc.DocumentNode.Descendants("li");
    var links = desc.Where(d => d.ChildNodes.Any() && d.ChildNodes.First().Name.Equals("a", StringComparison.OrdinalIgnoreCase));
    foreach (var htmlNode in links)
    {
        var link = htmlNode.FirstChild.Attributes["href"].Value;
        var fullUri = new System.Uri(pageUri, link);
        var name = CleanupString(htmlNode.FirstChild.InnerText);
        var sb = new StringBuilder();
        for (int i = 1; i < htmlNode.ChildNodes.Count; i++)
        {
            sb.Append(htmlNode.ChildNodes[i].InnerText);
        }
        var afterLinkText = CleanupString(sb.ToString());
        var pattern = "\\s*-\\s*(?\'Status\'[^-]*)(\\s*-\\s*)?(?\'Info\'.*)$";
        var match = Regex.Match(afterLinkText, pattern);
        var status = match.Groups["Status"].Value;
        var info = match.Groups["Info"].Value;
        var scrapedRezoning = new Rezoning() { Name = name, Status = status, Info = info };
        var rezoningsWithSameName = rezonings.Where(r => r.Name.Equals(name, StringComparison.OrdinalIgnoreCase));
        if (rezoningsWithSameName.Any())
        {
            log.Info($"Existing entry for {name} found.");
            var oldVersion = rezoningsWithSameName.Single();
            var changeDetails = new List<string>();
            if(!scrapedRezoning.Status.Equals(oldVersion.Status, StringComparison.OrdinalIgnoreCase))
            {
                if(String.IsNullOrEmpty(oldVersion.Status))
                   changeDetails.Add($"New status: *{scrapedRezoning.Status}*");
                else
                   changeDetails.Add($"Status changed from *{oldVersion.Status}* to *{scrapedRezoning.Status}*");
            }
            if (!scrapedRezoning.Info.Equals(oldVersion.Info, StringComparison.OrdinalIgnoreCase))
            {
                if(String.IsNullOrEmpty(oldVersion.Info))
                   changeDetails.Add($"New info: *{scrapedRezoning.Info}*");
                else
                   changeDetails.Add($"Detail changed from *{oldVersion.Info}* to *{scrapedRezoning.Info}*");
            }
            if(changeDetails.Any())
            {
                oldVersion.Status = scrapedRezoning.Status;
                oldVersion.Info = scrapedRezoning.Info;
                UpdateRezoningInDb(oldVersion);
                var message = $"Rezoning application updated: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\n";
                message += String.Join("\n", changeDetails);
                SendMessageToSlack(message, log);
            }
        }
        else
        {
            log.Info($"Writing new rezoning with name='{scrapedRezoning.Name}', Status='{scrapedRezoning.Status}', Info='{scrapedRezoning.Info}' ");
            InsertRezoningToDb(scrapedRezoning);
            var message = $"New rezoning application: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\nStatus: {scrapedRezoning.Status}";
            if (!String.IsNullOrEmpty(scrapedRezoning.Info))
                message += $"\n{scrapedRezoning.Info}";
            SendMessageToSlack(message, log);
            log.Info("Wrote to DB");
        }
    }

}

private static void SendMessageToSlack(string message, TraceWriter log)
{
    log.Info($"Sending message to Slack: {message}");
    var client = new HttpClient();
    var content = new StringContent($"{{\"text\":\"{message}\"}}", Encoding.UTF8, "application/json");
    var response = client.PostAsync(ConfigurationManager.AppSettings["SlackMessageUri"], content).Result;
    if (!response.IsSuccessStatusCode)
    {
        throw new Exception($"Failed to send message to slack: {response}");
    }
}
private static List<Rezoning> GetRezoningsFromDb()
{
    using (var db = new SqlConnection(ConfigurationManager.ConnectionStrings["SqlServerConnString"].ConnectionString))
    {
        return db.Query<Rezoning>("select Id, Name, Status, Info from Rezonings").AsList<Rezoning>();
    }
}
private static void InsertRezoningToDb(Rezoning r)
{
    using (var db = new SqlConnection(ConfigurationManager.ConnectionStrings["SqlServerConnString"].ConnectionString))
    {
        db.Insert(r);
    }
}

private static void UpdateRezoningInDb(Rezoning r)
{
    using (var db = new SqlConnection(ConfigurationManager.ConnectionStrings["SqlServerConnString"].ConnectionString))
    {
        db.Update(r);
    }
}

private static string CleanupString(string input)
{
    return RemoveLineBreaksAndExtraWhiteSpace(WebUtility.HtmlDecode(input)).Trim();
}

private static string RemoveLineBreaksAndExtraWhiteSpace(string input)
{
    var ret = input.Replace("\\r\\n", "");
    ret = ret.Replace("\\n", "");
    ret = Regex.Replace(ret, "\\s+", " ");
    return ret;
}