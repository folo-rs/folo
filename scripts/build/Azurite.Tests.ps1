#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for Azurite.psm1. Test-AzuriteReachable is exercised against a real ephemeral
# TcpListener (up) and a closed port (down); New-AzuriteCertificate is asked to produce a PFX which
# is then loaded back and inspected; Get-AzuriteArgument is asserted flag-for-flag; and
# Stop-AzuriteProcessTree is driven with -WhatIf (including a mocked child process on Windows) so it
# classifies the whole tree without killing anything, and once without -WhatIf to confirm it stops
# both the root and its descendant.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Azurite.psm1') -Force
}

Describe 'Test-AzuriteReachable' {
    It 'returns $true when a listener is accepting connections' {
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
        $listener.Start()
        try {
            $port = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
            Test-AzuriteReachable -BlobHost '127.0.0.1' -BlobPort $port | Should -BeTrue
        } finally {
            $listener.Stop()
        }
    }

    It 'returns $false when nothing is listening' {
        # Port 1 is effectively never open on a loopback interface, so the connect is refused.
        Test-AzuriteReachable -BlobHost '127.0.0.1' -BlobPort 1 | Should -BeFalse
    }
}

Describe 'New-AzuriteCertificate' {
    It 'writes a loadable PFX for CN=127.0.0.1 with a long lifetime' {
        $pfxPath = Join-Path $TestDrive 'selfsigned.pfx'
        New-AzuriteCertificate -PfxPath $pfxPath -Password 'test-pass'

        Test-Path -LiteralPath $pfxPath | Should -BeTrue

        $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($pfxPath, 'test-pass')
        try {
            $cert.Subject | Should -Be 'CN=127.0.0.1'
            ($cert.NotAfter - $cert.NotBefore).TotalDays | Should -BeGreaterThan 3000
        } finally {
            $cert.Dispose()
        }
    }

    It 'does nothing under -WhatIf' {
        $pfxPath = Join-Path $TestDrive 'whatif.pfx'
        New-AzuriteCertificate -PfxPath $pfxPath -Password 'test-pass' -WhatIf
        Test-Path -LiteralPath $pfxPath | Should -BeFalse
    }
}

Describe 'Get-AzuriteArgument' {
    It 'builds the HTTPS + OAuth in-memory emulator argument vector' {
        $argv = Get-AzuriteArgument -BlobHost '127.0.0.1' -BlobPort 10000 -CertPath '/tmp/c.pfx' -Password 'pw'
        $expected = @(
            '--blobHost', '127.0.0.1',
            '--blobPort', '10000',
            '--cert', '/tmp/c.pfx',
            '--pwd', 'pw',
            '--oauth', 'basic',
            '--inMemoryPersistence',
            '--skipApiVersionCheck',
            '--silent',
            '--loose'
        )
        ($argv -join ' ') | Should -Be ($expected -join ' ')
    }

    It 'stringifies the port number' {
        $argv = Get-AzuriteArgument -BlobHost 'localhost' -BlobPort 12345 -CertPath 'c' -Password 'pw'
        $portIndex = [array]::IndexOf($argv, '--blobPort')
        $argv[$portIndex + 1] | Should -BeOfType [string]
        $argv[$portIndex + 1] | Should -Be '12345'
    }
}

Describe 'Stop-AzuriteProcessTree' {
    It 'does not throw for an unknown process id under -WhatIf' {
        { Stop-AzuriteProcessTree -ProcessId 2147483647 -WhatIf } | Should -Not -Throw
    }

    It 'never stops any process in the tree under -WhatIf' -Skip:(-not $IsWindows) {
        # The child-enumeration recursion is Windows-only (Get-CimInstance is a CIM cmdlet that does
        # not exist on Linux), so this runs only on Windows. Hand it one synthetic child and assert
        # -WhatIf reaches all the way down the tree: no Stop-Process anywhere.
        InModuleScope Azurite {
            Mock Get-CimInstance {
                if ($Filter -eq 'ParentProcessId = 4242') {
                    [pscustomobject]@{ ProcessId = 9191 }
                }
            }
            Mock Stop-Process { }

            Stop-AzuriteProcessTree -ProcessId 4242 -WhatIf

            Should -Invoke Stop-Process -Times 0 -Exactly
            Should -Invoke Get-CimInstance -Times 2 -Exactly
        }
    }

    It 'stops every process in the tree without -WhatIf' -Skip:(-not $IsWindows) {
        InModuleScope Azurite {
            Mock Get-CimInstance {
                if ($Filter -eq 'ParentProcessId = 4242') {
                    [pscustomobject]@{ ProcessId = 9191 }
                }
            }
            Mock Stop-Process { }

            Stop-AzuriteProcessTree -ProcessId 4242

            Should -Invoke Stop-Process -Times 1 -Exactly -ParameterFilter { $Id -eq 4242 }
            Should -Invoke Stop-Process -Times 1 -Exactly -ParameterFilter { $Id -eq 9191 }
        }
    }
}
