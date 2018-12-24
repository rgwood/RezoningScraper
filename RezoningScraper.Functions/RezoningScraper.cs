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
using Microsoft.Extensions.Logging;
using Microsoft.WindowsAzure.Storage;
using Microsoft.WindowsAzure.Storage.Blob;
using Microsoft.WindowsAzure.Storage.Table;
using System.Threading.Tasks;

namespace AbundantHousingVancouver
{
    public static class RezoningScraper
    {
        private static ILogger Log;
        private static string StorageConnectionString;
        private static string SlackMessageUri;
        private static string RezoningPageUri;
        private static bool IsRunningInTestMode;

        private static CloudStorageAccount StorageAccount;
        private static CloudTable RezoningsTable;

        [FunctionName("RezoningScraper")]
        public async static Task Run([TimerTrigger("%TimerSchedule%", RunOnStartup = true)]TimerInfo myTimer, ILogger log, ExecutionContext context)
        {
            setUpLogging(log);
            loadConfiguration(context);
            await loadAzureStorageObjects();

            var webpageHtml = new HtmlWeb().Load(RezoningPageUri);
            saveHtmlToBlobStorage(StorageAccount, webpageHtml);

            var existingRezonings = await GetRezoningsFromDb(RezoningsTable);
            parseHtmlAndProcessUpdates(webpageHtml, existingRezonings);
        }

        private static void setUpLogging(ILogger _log)
        {
            Log = _log;
        }

        private static void loadConfiguration(ExecutionContext context)
        {
            var config = new ConfigurationBuilder()
            .SetBasePath(context.FunctionAppDirectory)
            .AddJsonFile("local.settings.json", optional: true, reloadOnChange: true)
            .AddEnvironmentVariables()
            .Build();
            IsRunningInTestMode = Boolean.Parse(config["TestMode"] ?? "false");
            StorageConnectionString = config["StorageConnectionString"];
            RezoningPageUri = config["RezoningPageUri"];
            SlackMessageUri = config["SlackMessageUri"];

            if (IsRunningInTestMode)
            {
                Log.LogInformation("Running in test mode");
            }
        }

        private async static Task loadAzureStorageObjects()
        {
            StorageAccount = CloudStorageAccount.Parse(StorageConnectionString);
            RezoningsTable = await getRezoningsTableAndCreateIfDoesntExist(StorageAccount);
        }

        private async static Task<CloudTable> getRezoningsTableAndCreateIfDoesntExist(CloudStorageAccount storageAccount)
        {
            CloudTableClient tableClient = storageAccount.CreateCloudTableClient();
            CloudTable table = tableClient.GetTableReference("rezonings");
            await table.CreateIfNotExistsAsync();
            return table;
        }

        private static void saveHtmlToBlobStorage(CloudStorageAccount storageAccount, HtmlDocument doc)
        {
            var saveFileName = $"VancouverRezoningWebpage-{DateTime.UtcNow.ToString("yyyyMMddTHHmmss")}.html";
            CloudBlobClient blobClient = storageAccount.CreateCloudBlobClient();
            CloudBlobContainer container = blobClient.GetContainerReference("rezoning-scrapes");
            SaveTextFileToAzure(container, doc.DocumentNode.OuterHtml, saveFileName);
            Log.LogInformation($"Saved HTML to {saveFileName}");
        }
        private async static void SaveTextFileToAzure(CloudBlobContainer container, string fileContents, string fileName)
        {
            CloudBlockBlob blockBlob = container.GetBlockBlobReference(fileName);
            await blockBlob.UploadTextAsync(fileContents);
        }

        private static void SendMessageToSlack(string message)
        {
            Log.LogInformation($"Sending message to Slack: {message}");
            if (IsRunningInTestMode)
                return;
            var client = new HttpClient();
            var content = new StringContent($"{{\"text\":\"{message}\"}}", Encoding.UTF8, "application/json");
            var response = client.PostAsync(SlackMessageUri, content).Result;
            if (!response.IsSuccessStatusCode)
            {
                throw new Exception($"Failed to send message to slack: {response}");
            }
        }
        private async static Task<List<Rezoning>> GetRezoningsFromDb(CloudTable table)
        {
            var ret = new List<Rezoning>();
            var query = new TableQuery<Rezoning>();
            TableContinuationToken token = null;

            do
            {

                TableQuerySegment<Rezoning> seg = await table.ExecuteQuerySegmentedAsync<Rezoning>(query, token);
                token = seg.ContinuationToken;
                ret.AddRange(seg);

            } while (token != null);

            return ret;
        }

        // The big ugly HTML parser. This should really be rewritten, but it's closely tied to the CoV formatting 
        // which could change at any time, so it's probably not worth spending time cleaning it up
        private static void parseHtmlAndProcessUpdates(HtmlDocument html, List<Rezoning> existingRezonings)
        {
            var box = html.DocumentNode.SelectNodes("//div[@class='box']").Single();
            var nodesUpToHeritage = box.ChildNodes.TakeWhile(n => !n.DescendantsAndSelf().Any(n1 => n1.InnerText.Equals("Heritage Application Legal Information")));
            
            var desc = nodesUpToHeritage.Select(n => n.SelectNodes("li")).Where(n => n != null).SelectMany(n => n).ToList();
            var links = desc.Where(d => d.ChildNodes.Any() && d.ChildNodes.First().Name.Equals("a", StringComparison.OrdinalIgnoreCase));
            foreach (var htmlNode in links)
            {
                var link = htmlNode.FirstChild.Attributes["href"].Value;
                var fullUri = new Uri(new Uri(RezoningPageUri), link);
                var name = CleanupString(htmlNode.FirstChild.InnerText);
                var sb = new StringBuilder();
                for (int i = 1; i < htmlNode.ChildNodes.Count; i++)
                {
                    sb.Append(htmlNode.ChildNodes[i].InnerText);
                }
                var afterLinkText = CleanupString(sb.ToString());
                var parsed = ParsePostLinkString(afterLinkText);
                var scrapedRezoning = new Rezoning(name) { Name = name, Status = parsed.status, Info = parsed.info };

                var rezoningsWithSameName = existingRezonings.Where(r => r.Name.Equals(name, StringComparison.OrdinalIgnoreCase));
                if (rezoningsWithSameName.Any())
                {
                    Log.LogInformation($"Existing entry for {name} found.");
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
                        UpdateRezoningInDb(RezoningsTable, oldVersion);
                        var message = $"Rezoning application updated: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\n";
                        message += String.Join("\n", changeDetails);
                        SendMessageToSlack(message);
                    }
                }
                else
                {
                    Log.LogInformation($"Writing new rezoning with name='{scrapedRezoning.Name}', Status='{scrapedRezoning.Status}', Info='{scrapedRezoning.Info}' ");
                    InsertRezoningToDb(RezoningsTable, scrapedRezoning);
                    var message = $"New rezoning application: *<{fullUri.ToString()}|{scrapedRezoning.Name}>*\nStatus: {scrapedRezoning.Status}";
                    if (!String.IsNullOrEmpty(scrapedRezoning.Info))
                        message += $"\n{scrapedRezoning.Info}";
                    SendMessageToSlack(message);
                    Log.LogInformation("Wrote to DB");
                }
            }
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

        public static (string status, string info) ParsePostLinkString(string input)
        {
            var pattern = "\\s*-\\s*(?\'Status\'[^-]*)(\\s*-\\s*)?(?\'Info\'.*)$";
            var match = Regex.Match(input, pattern);
            var status = CleanupString(match.Groups["Status"].Value);
            var info = CleanupString(match.Groups["Info"].Value);
            return (status, info);
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