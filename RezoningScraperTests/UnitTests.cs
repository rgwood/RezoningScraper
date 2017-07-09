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
        [TestCase(" ", "")]
        [TestCase(" \r\n ", "")]
        [TestCase(" - ", "-")]
        public void TestStringCleanup(string input, string expectedOutput)
        {
            Assert.AreEqual(expectedOutput,RezoningScraper.CleanupString(input));
        }

        [TestCase(" - Approved", "Approved", "")]
        [TestCase(" - Approved - Open House", "Approved", "Open House")]
        [TestCase(" - Approved-Open House ", "Approved", "Open House")]
        [TestCase(" - Approved- Open House ", "Approved", "Open House")]
        [TestCase(" - Approved -Open House ", "Approved", "Open House")]
        public void TestRegex(string input, string expectedStatus, string expectedInfo)
        {
            var result = RezoningScraper.ParsePostLinkString(input);

            Assert.AreEqual(expectedStatus, result.Status);
            Assert.AreEqual(expectedInfo, result.Info);
        }

    }
}
