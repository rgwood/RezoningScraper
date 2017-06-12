#r "System.Collections"
#r "System.IO"

using System;
using BingMapsRESTToolkit;

public static void Run(TimerInfo myTimer, TraceWriter log, Stream outputBlob)
{
    //github
    log.Info($"C# Timer trigger function executed at: {DateTime.Now}");
    //Create an image request.
    var request = new ImageryRequest()
    {
        CenterPoint = new Coordinate(33.769340,-84.393599),
        ZoomLevel = 20,
        ImagerySet = ImageryType.Birdseye,
        BingMapsKey = "AmWPWL1QDJgNJU7ZQM8tTD-I_Wz8cenvxhdMTQ6LD4NWEMEB_3wC3D0FGRwoc1QS"
    };

    //Process the request by using the ServiceManager.
    using (var imageStream = ServiceManager.GetImageAsync(request).Result)
    {
        log.Info($"got image");
        //Do something with the image stream.
        imageStream.CopyTo(outputBlob);
        //var byteArray = imageStream.ToArray();
        //outBlob.Write(byteArray, 0, byteArray.Length);
    }
    
}
