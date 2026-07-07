#requires -Version 7

# Helpers for the `test-azurite` recipe, which runs the cargo-bench-history Azure-storage tests
# against a local Azurite blob emulator over HTTPS + OAuth.
#
# The recipe itself is imperative I/O (probe the port, maybe launch the emulator, wait, run tests,
# tear down in a finally). These helpers carve out the pieces worth isolating and testing: the
# TCP reachability probe, the recursive process-tree kill (the Windows `.cmd` shim spawns node as a
# child, so killing only the shim orphans the emulator), the self-signed certificate generation
# (Entra auth requires TLS, so `--oauth basic` only runs over HTTPS), and the emulator argument
# vector.

Set-StrictMode -Version Latest

function Test-AzuriteReachable {
    # True when something is already accepting TCP connections on the Azurite blob endpoint, used
    # both to decide whether to launch an emulator and to poll a freshly-launched one until ready.
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][string] $BlobHost,
        [Parameter(Mandatory)][int] $BlobPort
    )

    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect($BlobHost, $BlobPort)
        return $true
    } catch {
        return $false
    } finally {
        $client.Dispose()
    }
}

function Stop-AzuriteProcessTree {
    # Stop a process and (on Windows) its descendants. The Windows `.cmd` shim spawns node as a
    # child, so killing only the shim would orphan the emulator that actually holds the port.
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][int] $ProcessId
    )

    if ($IsWindows) {
        # Forward -WhatIf explicitly so the whole tree is classified, not just this node. Preference
        # inheritance ($WhatIfPreference/$ConfirmPreference) already flows into the recursion, but
        # making -WhatIf explicit keeps the intent unmistakable and independent of that subtlety.
        Get-CimInstance Win32_Process -Filter "ParentProcessId = $ProcessId" -ErrorAction SilentlyContinue |
            ForEach-Object {
                Stop-AzuriteProcessTree -ProcessId $_.ProcessId -WhatIf:$WhatIfPreference
            }
    }
    if ($PSCmdlet.ShouldProcess("process $ProcessId", 'Stop-Process')) {
        Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
    }
}

function New-AzuriteCertificate {
    # Generate a self-signed certificate for Azurite's HTTPS listener, written as a
    # password-protected PFX (the cross-platform format .NET exports). Entra auth requires TLS, so
    # `--oauth basic` only runs over HTTPS; the tests trust this cert by disabling certificate
    # validation, so its contents (beyond being a valid cert for 127.0.0.1) do not matter. It is
    # given a 10-year lifetime so the cached copy never needs rolling.
    [CmdletBinding(SupportsShouldProcess)]
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingPlainTextForPassword', '',
        Justification = 'Throwaway password for an ephemeral self-signed localhost test certificate whose contents are deliberately untrusted; not a secret.')]
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUsePSCredentialType', '',
        Justification = 'The .NET PFX export API takes a plain string; a PSCredential would add no protection for a throwaway test certificate password.')]
    param(
        [Parameter(Mandatory)][string] $PfxPath,
        [Parameter(Mandatory)][string] $Password
    )

    if (-not $PSCmdlet.ShouldProcess($PfxPath, 'Create self-signed certificate')) { return }

    $rsa = [System.Security.Cryptography.RSA]::Create(2048)
    $certificate = $null
    try {
        $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
            'CN=127.0.0.1',
            $rsa,
            [System.Security.Cryptography.HashAlgorithmName]::SHA256,
            [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
        $sanBuilder = [System.Security.Cryptography.X509Certificates.SubjectAlternativeNameBuilder]::new()
        $sanBuilder.AddIpAddress([System.Net.IPAddress]::Loopback)
        $sanBuilder.AddDnsName('localhost')
        $request.CertificateExtensions.Add($sanBuilder.Build())
        $notBefore = [System.DateTimeOffset]::UtcNow.AddDays(-1)
        $notAfter = [System.DateTimeOffset]::UtcNow.AddYears(10)
        $certificate = $request.CreateSelfSigned($notBefore, $notAfter)
        $pfxBytes = $certificate.Export(
            [System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx,
            $Password)
        [System.IO.File]::WriteAllBytes($PfxPath, $pfxBytes)
    } finally {
        # X509Certificate2 holds a native key handle, so dispose it (alongside the RSA key) rather
        # than waiting on the finalizer; test-azurite may generate this repeatedly in one session.
        if ($certificate) { $certificate.Dispose() }
        $rsa.Dispose()
    }
}

function Get-AzuriteArgument {
    # Build the Azurite blob-emulator argument vector: an in-memory HTTPS listener with OAuth
    # (`basic`) enabled, bound to the given host/port and using the given certificate. Isolated so
    # the exact flag set is asserted by tests rather than buried in the launch call.
    [CmdletBinding()]
    [OutputType([string[]])]
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingPlainTextForPassword', '',
        Justification = 'Throwaway password protecting an ephemeral self-signed localhost test certificate; not a secret.')]
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUsePSCredentialType', '',
        Justification = 'The value is forwarded verbatim as an Azurite `--pwd` CLI argument, which takes a plain string.')]
    param(
        [Parameter(Mandatory)][string] $BlobHost,
        [Parameter(Mandatory)][int] $BlobPort,
        [Parameter(Mandatory)][string] $CertPath,
        [Parameter(Mandatory)][string] $Password
    )

    return @(
        '--blobHost', $BlobHost,
        '--blobPort', "$BlobPort",
        '--cert', $CertPath,
        '--pwd', $Password,
        '--oauth', 'basic',
        '--inMemoryPersistence',
        '--skipApiVersionCheck',
        '--silent',
        '--loose'
    )
}

Export-ModuleMember -Function Test-AzuriteReachable, Stop-AzuriteProcessTree, New-AzuriteCertificate, Get-AzuriteArgument
