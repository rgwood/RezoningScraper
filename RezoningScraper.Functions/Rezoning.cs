using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using Microsoft.WindowsAzure.Storage.Table;

namespace AbundantHousingVancouver
{
    public class Rezoning : TableEntity
    {
        public readonly string DefaultPartitionKey = "default";
        public Rezoning(string name)
        {
            this.PartitionKey = DefaultPartitionKey;
            this.RowKey = name;
            Name = name;
        }

        public Rezoning() { }
        public string Name { get; set; }
        public string Status { get; set; }
        public string Info { get; set; }
    }
}
