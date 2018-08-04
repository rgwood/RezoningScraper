using System;
using Microsoft.Azure.WebJobs;
using Microsoft.Azure.WebJobs.Host;
using HtmlAgilityPack;
using System.Collections.Generic;
using System.Linq;
using System.Net;
using System.Text;
using System.Text.RegularExpressions;
using System.Net.Http;
using Microsoft.Azure;
using Microsoft.Extensions.Configuration;
using Microsoft.WindowsAzure.Storage;
using Microsoft.WindowsAzure.Storage.Blob;
using Microsoft.WindowsAzure.Storage.Table;
using System.Threading.Tasks;

namespace AbundantHousingVancouver
{
    public static class RezoningScraper
    {
        [FunctionName("RezoningScraper")]
        public async static Task Run([TimerTrigger("%TimerSchedule%", RunOnStartup = true)]TimerInfo myTimer, TraceWriter log, ExecutionContext context)
        //TEST public static void Run([TimerTrigger("*/10 * * * * *")]TimerInfo myTimer, TraceWriter log)
        {
            var TESTMODE = false;

            var config = new ConfigurationBuilder()
            .SetBasePath(context.FunctionAppDirectory)
            .AddJsonFile("local.settings.json", optional: true, reloadOnChange: true)
            .AddEnvironmentVariables()
            .Build();

            CloudStorageAccount storageAccount = CloudStorageAccount.Parse(config["StorageConnectionString"]);
            CloudTableClient tableClient = storageAccount.CreateCloudTableClient();
            CloudTable table = tableClient.GetTableReference("rezonings");
            await table.CreateIfNotExistsAsync();
            
            var rezonings = await GetRezoningsFromDb(table);

            var pageUri = new Uri(config["RezoningPageUri"]);
            var web = new HtmlWeb();
            var doc = web.Load(pageUri.ToString());

            //save the HTML for debugging purposes
            var saveFileName = $"VancouverRezoningWebpage-{DateTime.UtcNow.ToString("yyyyMMddTHHmmss")}.html";
            CloudBlobClient blobClient = storageAccount.CreateCloudBlobClient();
            CloudBlobContainer container = blobClient.GetContainerReference("rezoning-scrapes");
            SaveTextFileToAzure(container, doc.DocumentNode.OuterHtml, saveFileName);
            log.Info($"Saved HTML to {saveFileName}");
            
            var desc = doc.DocumentNode.Descendants("li");
            var links = desc.Where(d => d.ChildNodes.Any() && d.ChildNodes.First().Name.Equals("a", StringComparison.OrdinalIgnoreCase));
            foreach (var htmlNode in links)
            {
                var link = htmlNode.FirstChild.Attributes["href"].Value;
                var fullUri = new Uri(pageUri, link);
                var name = CleanupString(htmlNode.FirstChild.InnerText);
                var sb = new StringBuilder();
                for (int i = 1; i < htmlNode.ChildNodes.Count; i++)
                {
                    sb.Append(htmlNode.ChildNodes[i].InnerText);
                }
                var afterLinkText = CleanupString(sb.ToString());
                var parsed = ParsePostLinkString(afterLinkText);
                var scrapedRezoning = new Rezoning(name) { Name = name, Status = parsed.Status, Info = parsed.Info };
                
                var rezoningsWithSameName = rezonings.Where(r => r.Name.Equals(name, StringComparison.OrdinalIgnoreCase));
                if (rezoningsWithSameName.Any())
                {
                    log.Info($"Existing entry for {name} found.");
                    var oldVersion = rezoningsWithSameName.Single();
                    var changeDetails = new List<string>();
                    if (!scrapedRezoning.Status.Equals(oldVersion.Status, StringComparison.OrdinalIgnoreCase))
                    {
                        if (String.IsNullOrEmpty(oldVersion.Status))
                            changeDetails.Add($"New status: *{scrapedRezoning.Status}*");
                        else
                        {
                            if (String.IsNullOrEmpty(scrapedRezoning.Status))
                                changeDetails.Add($"Status *{oldVersion.Status}* was removed");
                            else
                                changeDetails.Add($"Status changed from *{oldVersion.Status}* to *{scrapedRezoning.Status}*");
                        }

                    }
                    if (!scrapedRezoning.Info.Equals(oldVersion.Info, StringComparison.OrdinalIgnoreCase))
                    {
                        if (String.IsNullOrEmpty(oldVersion.Info))
                            changeDetails.Add($"New info: *{scrapedRezoning.Info}*");
                        else
                        {
                            if (String.IsNullOrEmpty(scrapedRezoning.Info))
                                changeDetails.Add($"Detail *{oldVersion.Info}* was removed");
                            else
                                changeDetails.Add($"Detail changed from *{oldVersion.Info}* to *{scrapedRezoning.Info}*");
                        }
                    }
                    if (changeDetails.Any())
                    {
                        oldVersion.Status = scrapedRezoning.Status;
                        oldVersion.Info = scrapedRezoning.Info;
                        UpdateRezoningInDb(table, oldVersion);
                        var message = $"Rezoning application updated: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\n";
                        message += String.Join("\n", changeDetails);
                        SendMessageToSlack(config["SlackMessageUri"], message, log, TESTMODE);
                    }
                }
                else
                {
                    log.Info($"Writing new rezoning with name='{scrapedRezoning.Name}', Status='{scrapedRezoning.Status}', Info='{scrapedRezoning.Info}' ");
                    InsertRezoningToDb(table, scrapedRezoning);
                    var message = $"New rezoning application: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\nStatus: {scrapedRezoning.Status}";
                    if (!String.IsNullOrEmpty(scrapedRezoning.Info))
                        message += $"\n{scrapedRezoning.Info}";
                    SendMessageToSlack(config["SlackMessageUri"], message, log, TESTMODE);
                    log.Info("Wrote to DB");
                }
            }

        }
        
        private async static void SaveTextFileToAzure(CloudBlobContainer container, string fileContents, string fileName)
        {
            CloudBlockBlob blockBlob = container.GetBlockBlobReference(fileName);
            await blockBlob.UploadTextAsync(fileContents);
        }

        private static void SendMessageToSlack(string slackUri, string message, TraceWriter log, bool testMode)
        {
            log.Info($"Sending message to Slack: {message}");
            if (testMode)
                return;
            var client = new HttpClient();
            var content = new StringContent($"{{\"text\":\"{message}\"}}", Encoding.UTF8, "application/json");
            var response = client.PostAsync(slackUri, content).Result;
            if (!response.IsSuccessStatusCode)
            {
                throw new Exception($"Failed to send message to slack: {response}");
            }
        }
        private async static Task<List<Rezoning>> GetRezoningsFromDb(CloudTable table)
        {
            var query = new TableQuery<Rezoning>();

            var ret = new List<Rezoning>();
            TableContinuationToken token = null;

            do
            {

                TableQuerySegment<Rezoning> seg = await table.ExecuteQuerySegmentedAsync<Rezoning>(query, token);
                token = seg.ContinuationToken;
                ret.AddRange(seg);

            } while (token != null);

            return ret;
        }

        private async static void InsertRezoningToDb(CloudTable table, Rezoning r)
        {
            TableOperation insertOperation = TableOperation.Insert(r);
            await table.ExecuteAsync(insertOperation);
        }

        private async static void UpdateRezoningInDb(CloudTable table, Rezoning r)
        {
            TableOperation operation = TableOperation.Replace(r);
            
            await table.ExecuteAsync(operation);
        }

        //This shouldnt' be necessary but C#7 tuple syntax isn't compiling correctly for some reason
        public class RegexReturnType
        {
            public RegexReturnType(string status, string info)
            {
                Status = status;
                Info = info;
            }
            public string Status;
            public string Info;
        }

        public static RegexReturnType ParsePostLinkString(string input)
        {
            var pattern = "\\s*-\\s*(?\'Status\'[^-]*)(\\s*-\\s*)?(?\'Info\'.*)$";
            var match = Regex.Match(input, pattern);
            var status = CleanupString(match.Groups["Status"].Value);
            var info = CleanupString(match.Groups["Info"].Value);
            return new RegexReturnType(status, info);
        }

        public static string CleanupString(string input)
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
    }
}