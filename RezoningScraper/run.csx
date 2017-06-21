#r "System.Collections"
#r "System.IO"

using System;

public static void Run(TimerInfo myTimer, TraceWriter log)
{
    log.Info($"C# Timer trigger function executed at: {DateTime.Now}");
    log.Info(ConfigurationManager.AppSettings["SlackMessageUri"]);
    log.Info(ConfigurationManager.AppSettings["RezoningPageUri"]);
    log.Info(ConfigurationManager.ConnectionStrings["SqlServerConnString"].ConnectionString);
 
}
