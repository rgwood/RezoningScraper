using NUnit.Framework;
using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using AbundantHousingVancouver;

namespace RezoningScraperTests
{
    [TestFixture]
    public class UnitTests
    {
        [TestCase("","")]
        public void TestStringCleanup(string input, string expectedOutput)
        {
            Assert.AreEqual(RezoningScraper.CleanupString(input),"");
        }
    }
}
