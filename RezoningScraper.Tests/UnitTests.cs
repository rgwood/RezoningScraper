using Microsoft.VisualStudio.TestTools.UnitTesting;
using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using AbundantHousingVancouver;

namespace RezoningScraperTests
{
    [TestClass]
    public class UnitTests
    {
        [TestMethod]
        [DataRow("","")]
        [DataRow(" ", "")]
        [DataRow(" \r\n ", "")]
        [DataRow(" - ", "-")]
        public void TestStringCleanup(string input, string expectedOutput)
        {
            Assert.AreEqual(expectedOutput,RezoningScraper.CleanupString(input));
        }

        [TestMethod]
        [DataRow(" - Approved", "Approved", "")]
        [DataRow(" - Approved - Open House", "Approved", "Open House")]
        [DataRow(" - Approved-Open House ", "Approved", "Open House")]
        [DataRow(" - Approved- Open House ", "Approved", "Open House")]
        [DataRow(" - Approved -Open House ", "Approved", "Open House")]
        public void TestRegex(string input, string expectedStatus, string expectedInfo)
        {
            var result = RezoningScraper.ParsePostLinkString(input);

            Assert.AreEqual(expectedStatus, result.status);
            Assert.AreEqual(expectedInfo, result.info);
        }
    }
}
