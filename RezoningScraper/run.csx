#r "System.Collections"
#r "System.IO"
#r "Microsoft.WindowsAzure.Storage" // Namespace for CloudStorageAccount

using HtmlAgilityPack;
using System;
using System.Configuration;
using System.Collections.Generic;
using System.Linq;
using System.Net;
using System.Text;
using System.Text.RegularExpressions;
using System.Net.Http;
using Microsoft.Azure;
using Microsoft.WindowsAzure.Storage;
using Microsoft.WindowsAzure.Storage.Table;

private class Rezoning : TableEntity
{
    public readonly string DefaultPartitionKey = "default";
    public Rezoning(string name)
    {
        this.PartitionKey = DefaultPartitionKey;
        this.RowKey = name;
        Name = name;
    }

    public Rezoning(){}
    public string Name { get; set; }
    public string Status { get; set; }
    public string Info { get; set; }
}

public static void Run(TimerInfo myTimer, TraceWriter log)
{
    var TESTMODE = false;

    log.Info($"C# Timer trigger function executed at: {DateTime.Now}");

    CloudStorageAccount storageAccount = CloudStorageAccount.Parse(CloudConfigurationManager.GetSetting("StorageConnectionString"));
    CloudTableClient tableClient = storageAccount.CreateCloudTableClient();
    CloudTable table = tableClient.GetTableReference("rezonings");
    table.CreateIfNotExists();

    var rezonings = GetRezoningsFromDb(table);
    
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
        var status = CleanupString(match.Groups["Status"].Value);
        var info = CleanupString(match.Groups["Info"].Value);
        var scrapedRezoning = new Rezoning(name) { Name = name, Status = status, Info = info };
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
                {
                    if(String.IsNullOrEmpty(scrapedRezoning.Status))
                        changeDetails.Add($"Status *{oldVersion.Status}* was removed");
                    else
                        changeDetails.Add($"Status changed from *{oldVersion.Status}* to *{scrapedRezoning.Status}*");
                }

            }
            if (!scrapedRezoning.Info.Equals(oldVersion.Info, StringComparison.OrdinalIgnoreCase))
            {
                if(String.IsNullOrEmpty(oldVersion.Info))
                   changeDetails.Add($"New info: *{scrapedRezoning.Info}*");
                else
                {
                    if(String.IsNullOrEmpty(scrapedRezoning.Info))
                        changeDetails.Add($"Detail *{oldVersion.Info}* was removed");
                    else
                        changeDetails.Add($"Detail changed from *{oldVersion.Info}* to *{scrapedRezoning.Info}*");
                }
            }
            if(changeDetails.Any())
            {
                oldVersion.Status = scrapedRezoning.Status;
                oldVersion.Info = scrapedRezoning.Info;
                UpdateRezoningInDb(table, oldVersion);
                var message = $"Rezoning application updated: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\n";
                message += String.Join("\n", changeDetails);
                SendMessageToSlack(message, log, TESTMODE);
            }
        }
        else
        {
            log.Info($"Writing new rezoning with name='{scrapedRezoning.Name}', Status='{scrapedRezoning.Status}', Info='{scrapedRezoning.Info}' ");
            InsertRezoningToDb(table, scrapedRezoning);
            var message = $"New rezoning application: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\nStatus: {scrapedRezoning.Status}";
            if (!String.IsNullOrEmpty(scrapedRezoning.Info))
                message += $"\n{scrapedRezoning.Info}";
            SendMessageToSlack(message, log, TESTMODE);
            log.Info("Wrote to DB");
        }
    }

}

private static void SendMessageToSlack(string message, TraceWriter log, bool testMode)
{
    log.Info($"Sending message to Slack: {message}");
    if(testMode)
        return;
    var client = new HttpClient();
    var content = new StringContent($"{{\"text\":\"{message}\"}}", Encoding.UTF8, "application/json");
    var response = client.PostAsync(ConfigurationManager.AppSettings["SlackMessageUri"], content).Result;
    if (!response.IsSuccessStatusCode)
    {
        throw new Exception($"Failed to send message to slack: {response}");
    }
}
private static List<Rezoning> GetRezoningsFromDb(CloudTable table)
{
    var query = new TableQuery<Rezoning>();
    return table.ExecuteQuery(query).ToList();
}
private static void InsertRezoningToDb(CloudTable table, Rezoning r)
{
    TableOperation insertOperation = TableOperation.Insert(r);
    table.Execute(insertOperation);
}

private static void UpdateRezoningInDb(CloudTable table, Rezoning r)
{
    TableOperation operation = TableOperation.Replace(r);
    table.Execute(operation);
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