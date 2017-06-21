#r "System.Collections"
#r "System.IO"

using System;

public static void Run(TimerInfo myTimer, TraceWriter log, Stream outputBlob)
{
    log.Info($"C# Timer trigger function executed at: {DateTime.Now}");
 
}
