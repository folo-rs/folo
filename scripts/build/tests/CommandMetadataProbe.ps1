# Reproduces PSScriptAnalyzer's off-pipeline CommandInfo access using the real PowerShell runtime.
# Run manually with -Concurrent to expose the race; the integration suite runs the serial control.
# C# is needed for off-pipeline threads without introducing additional PowerShell runspaces.
# Ref: ../../../docs/build-and-tooling.md#powershell-linting.
[CmdletBinding()]
param(
    [switch] $Concurrent,
    # A bounded workload for overlapping metadata queries, not a timeout or retry policy.
    [ValidateRange(1, 10000)][int] $Iterations = 500
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Concurrent;
using System.Management.Automation;
using System.Management.Automation.Runspaces;
using System.Threading;
using System.Threading.Tasks;

namespace Folo.ScriptAnalysis.Tests
{
    // Models the analyzer's pooled command lookup followed by off-pipeline parameter access.
    public static class CommandMetadataProbe
    {
        public static string[] Run(bool concurrent, int iterations)
        {
            // Match the pinned analyzer's CommandInfoCache runspace-pool bounds.
            using var pool = RunspaceFactory.CreateRunspacePool(1, 10);
            pool.Open();
            using var lookup = PowerShell.Create();
            lookup.RunspacePool = pool;
            lookup.AddCommand("Get-Command").AddParameter("Name", "Export-ModuleMember");
            CommandInfo export = lookup.Invoke<CommandInfo>()[0];
            lookup.Commands.Clear();
            lookup.AddCommand("Get-Command").AddParameter("Name", "Get-ChildItem");
            CommandInfo dynamicCommand = lookup.Invoke<CommandInfo>()[0];
            var failures = new ConcurrentQueue<string>();
            using var barrier = new Barrier(2);

            Action resolveExport = () => {
                try
                {
                    if (export.ResolveParameter("Function").Name != "Function")
                        failures.Enqueue("Export-ModuleMember returned incorrect parameter metadata.");
                }
                catch (Exception error) { failures.Enqueue(error.ToString()); }
            };
            Action resolveDynamic = () => {
                try
                {
                    if (!dynamicCommand.Parameters.ContainsKey("Path"))
                        failures.Enqueue("Get-ChildItem returned incorrect parameter metadata.");
                }
                catch (Exception error) { failures.Enqueue(error.ToString()); }
            };
            Action query = () => {
                lookup.Invoke();
                if (lookup.HadErrors)
                    throw new InvalidOperationException(lookup.Streams.Error[0].ToString());
            };

            if (concurrent)
            {
                // A busy pooled runspace routes metadata requests through its event manager.
                // No sleeps or artificial metadata values are involved.
                Task.WaitAll(
                    Task.Factory.StartNew(() => {
                        for (int i = 0; i < iterations; i++) query();
                    }, TaskCreationOptions.LongRunning),
                    Task.Factory.StartNew(() => {
                        for (int i = 0; i < iterations; i++)
                        {
                            barrier.SignalAndWait();
                            resolveExport();
                        }
                    }, TaskCreationOptions.LongRunning),
                    Task.Factory.StartNew(() => {
                        for (int i = 0; i < iterations; i++)
                        {
                            barrier.SignalAndWait();
                            resolveDynamic();
                        }
                    }, TaskCreationOptions.LongRunning));
            }
            else
            {
                Task.Factory.StartNew(() => {
                    for (int i = 0; i < iterations; i++)
                    {
                        query();
                        resolveExport();
                        resolveDynamic();
                    }
                }, TaskCreationOptions.LongRunning).Wait();
            }
            return failures.ToArray();
        }
    }
}
'@

$failures = [Folo.ScriptAnalysis.Tests.CommandMetadataProbe]::Run($Concurrent.IsPresent, $Iterations)
[pscustomobject]@{
    Concurrent = $Concurrent.IsPresent
    Iterations = $Iterations
    Failures = $failures
}
if ($failures.Count -gt 0) {
    foreach ($failure in $failures) { Write-Host $failure }
    throw "Command metadata probe encountered $($failures.Count) failures."
}
