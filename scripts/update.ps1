#Requires -Version 5.1
<#
    Celira updater (Windows, the PACKAGED form).

        .\scripts\update.ps1 -Check                  # only ask: is there a newer package?
        .\scripts\update.ps1                         # channel (github) -> verify -> stop -> back up -> unpack
        .\scripts\update.ps1 -From D:\celira-0.1.3-win-x64.zip -Sha256 <hex>
        .\scripts\update.ps1 -SelfTest               # the script's own assertions, no network, no home

    Why the order is fixed (each step is a measured reason, not a preference):

      1. ask the channel    `probe-release-channel.mjs --channel github` -- atom + the asset page. No token,
                            no API quota, no gh. The hash it prints is the ONLY thing that can prove the
                            bytes; when the channel prints none, this script stops rather than pretend.
      2. the app must be    the shell owns its folder, and Windows will not let a running
         gone               `celira-app.exe` be replaced. Tray -> Quit first; closing the window only
                            hides it (the services keep running).
      3. home.js down       stops this home's world and residents.
      4. home.js backup     BEFORE the new version ever boots. Migrations only go up: once a newer
                            im server has written a pending migration, the OLD binary refuses to start
                            (sqlx VersionMissing). So the way back is the HOME, not the program folder.
                            Backup refuses while anything is running (exit 3) -- that refusal is a gate,
                            not an obstacle.
      5. unpack to a NEW    never overwrite the folder being replaced: that folder is also the way back.
         directory
      6. hand over          the new folder is ready; you close the old one and double-click the new exe.
                            The home is untouched, so you are still you and she is still her.

    What this script does NOT do: start the new program, delete the old folder, touch the home's
    contents, or call a download "verified" when the channel gave no hash.

    Exit codes: 0 = done (updated, already latest, or -Check answered), 1 = usage,
                2 = a prerequisite is missing, 3 = needs you (app still running / backup refused /
                unverifiable download), 4 = channel, download or hash failed, 5 = unpack failed.

    ASCII only, on purpose: Windows PowerShell 5.1 reads a BOM-less .ps1 with the system code page,
    and a non-ASCII character inside a string has already broken one script in this repo (see the
    header of make_release.ps1). The self-check below enforces it before anything else runs.
#>
[CmdletBinding()]
param(
    # A local .zip, or an https URL. Skip the channel entirely.
    [string]$From,

    # The hash to check the download against (64 hex chars). Required when the channel gives none.
    [string]$Sha256,

    # github | gitee | a fixture path (see probe-release-channel.mjs).
    [string]$Channel = 'github',

    # owner/name; defaults to the origin of this checkout (packages have no git remote).
    [string]$Repo,

    # The package directory to replace. Defaults to the parent of this script.
    [string]$Package,

    # The home to stop / back up. Defaults to CELIRA_HOME_DIR, then the platform default.
    [string]$HomeDir,

    # The version you are running, for a SOURCE checkout (no VERSION file). Packages read their own.
    [string]$Current,

    # Ask and report only: no download, no stop, no backup, no unpacking.
    [switch]$Check,

    # Print the plan, change nothing.
    [switch]$DryRun,

    # Say out loud that this run cannot prove the bytes (only when no hash exists anywhere).
    [switch]$AllowUnverified,

    # Run this script's own assertions instead of updating.
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# 0) this file must stay ASCII (its own rule, checked before anything else)
# ---------------------------------------------------------------------------
$selfBytes = [System.IO.File]::ReadAllBytes($PSCommandPath)
for ($i = 0; $i -lt $selfBytes.Length; $i++) {
    if ($selfBytes[$i] -gt 0x7F) {
        Write-Host ('[fail] this script is not ASCII-only (first offending byte at offset {0})' -f $i)
        exit 1
    }
}

$EXIT = @{ OK = 0; USAGE = 1; PREREQ = 2; NEEDS_YOU = 3; CHANNEL = 4; UNPACK = 5 }

# ---------------------------------------------------------------------------
# helpers (all ASCII, all side-effect free unless named Invoke-*)
# ---------------------------------------------------------------------------

function Read-Version {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {
        if ($line -match '^\s*version\s*=\s*(\S+)\s*$') { return $Matches[1] }
    }
    return $null
}

function Get-HashOf {
    param([string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Resolve-HomeDir {
    param([string]$Explicit)
    if ($Explicit -and $Explicit.Trim() -ne '') { return $Explicit }
    if ($env:CELIRA_HOME_DIR -and $env:CELIRA_HOME_DIR.Trim() -ne '') { return $env:CELIRA_HOME_DIR }
    return $null
}

# The hash policy is a pure decision so -SelfTest can assert it without touching a network or a home.
function Test-HashPolicy {
    param([string]$Given, [switch]$UnverifiedAllowed)
    if ($Given -and $Given.Trim() -ne '') { return @{ ok = $true; why = 'given' } }
    if ($UnverifiedAllowed) { return @{ ok = $true; why = 'unverified-allowed' } }
    return @{ ok = $false; why = 'no hash anywhere: pass -Sha256, or say -AllowUnverified out loud' }
}

function New-UpdateDirectory {
    param([string]$Parent, [string]$Name, [string]$Stamp)
    $base = Join-Path $Parent $Name
    $candidate = '{0}-{1}' -f $base, $Stamp
    $n = 1
    while (Test-Path -LiteralPath $candidate) {
        $n += 1
        $candidate = '{0}-{1}-{2}' -f $base, $Stamp, $n
    }
    return $candidate
}

function Invoke-NodeIn {
    param([string]$Where, [string[]]$Arguments)
    Push-Location -LiteralPath $Where
    try {
        & node @Arguments | Out-Host
        return $LASTEXITCODE
    } finally {
        Pop-Location
    }
}

# ---------------------------------------------------------------------------
# 0b) -SelfTest: the script's own assertions (no network, no home, no package)
# ---------------------------------------------------------------------------
if ($SelfTest) {
    $passed = 0
    $failed = 0
    function Assert-That {
        param([string]$What, [bool]$Condition)
        if ($Condition) { $script:passed += 1; Write-Host ('[selftest] ok   ' + $What) }
        else { $script:failed += 1; Write-Host ('[selftest] FAIL ' + $What) }
    }

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('celira-update-selftest-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        $versionFile = Join-Path $tmp 'VERSION'
        Set-Content -LiteralPath $versionFile -Encoding ascii -Value @('version = 9.9.9', 'git rev = abc1234', 'platform = windows-x64')
        Assert-That 'VERSION: the version row is read' ((Read-Version $versionFile) -eq '9.9.9')

        Set-Content -LiteralPath $versionFile -Encoding ascii -Value @('git rev = abc1234')
        Assert-That 'VERSION: without a version row it answers $null (never a made-up number)' ($null -eq (Read-Version $versionFile))

        Assert-That 'VERSION: a missing file answers $null' ($null -eq (Read-Version (Join-Path $tmp 'nope')))

        $probeFile = Join-Path $tmp 'bytes.bin'
        [System.IO.File]::WriteAllBytes($probeFile, [byte[]](1, 2, 3, 4, 5))
        $known = (Get-HashOf $probeFile)
        Assert-That 'hash: is 64 lowercase hex' ($known -match '^[0-9a-f]{64}$')
        Assert-That 'hash: is stable across calls' ((Get-HashOf $probeFile) -eq $known)
        Set-Content -LiteralPath $probeFile -Encoding ascii -Value 'different'
        Assert-That 'hash: changes with the bytes' ((Get-HashOf $probeFile) -ne $known)

        Assert-That 'hash policy: a given hash passes' ((Test-HashPolicy -Given 'deadbeef').ok)
        Assert-That 'hash policy: no hash and no permission refuses' (-not (Test-HashPolicy -Given '').ok)
        Assert-That 'hash policy: no hash but -AllowUnverified passes and says why' (
            (Test-HashPolicy -Given '' -UnverifiedAllowed).why -eq 'unverified-allowed'
        )

        $first = New-UpdateDirectory -Parent $tmp -Name 'celira-9.9.9-win-x64' -Stamp '20260101-000000'
        New-Item -ItemType Directory -Force -Path $first | Out-Null
        $second = New-UpdateDirectory -Parent $tmp -Name 'celira-9.9.9-win-x64' -Stamp '20260101-000000'
        Assert-That 'new directory: never reuses an existing folder (that folder is the way back)' ($second -ne $first)
        Assert-That 'new directory: stays under the parent' ($second.StartsWith($tmp))

        Assert-That 'home: an explicit -HomeDir wins' ((Resolve-HomeDir -Explicit 'D:\x') -eq 'D:\x')
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }

    Write-Host ('[selftest] ' + $passed + ' passed, ' + $failed + ' failed')
    if ($failed -gt 0) { exit 1 }
    if ($passed -le 0) { Write-Host '[selftest] no assertions ran -- that is a failure, not a pass'; exit 1 }
    exit $EXIT.OK
}

# ---------------------------------------------------------------------------
# 1) the package we are replacing
# ---------------------------------------------------------------------------
$packageDir = if ($Package) { (Resolve-Path -LiteralPath $Package).Path } else { Split-Path -Parent $PSScriptRoot }
if (-not (Test-Path -LiteralPath (Join-Path $packageDir 'home.js'))) {
    Write-Host ('[fail] no home.js in {0} -- run this from <package>\scripts\update.ps1' -f $packageDir)
    exit $EXIT.PREREQ
}
$packageName = Split-Path -Leaf $packageDir
Write-Host 'celira-update v1'
Write-Host ('package = {0}' -f $packageDir)

$running = Read-Version (Join-Path $packageDir 'VERSION')
$currentVersion = if ($Current) { $Current } elseif ($running) { $running } else { $null }
if ($currentVersion) { Write-Host ('current = {0}{1}' -f $currentVersion, $(if (-not $Current) { '   (from VERSION)' } else { '   (-Current)' })) }
else { Write-Host 'current = (unknown: no VERSION here and no -Current)' }
Write-Host ''

# ---------------------------------------------------------------------------
# 2) what does the channel say / what did the caller hand us
# ---------------------------------------------------------------------------
$plan = @{ source = $null; path = $null; url = $null; hash = $null; latest = $null; hasUpdate = $null; why = $null }

if ($From) {
    $plan.source = 'explicit'
    if ($From -match '^https?://') { $plan.url = $From } else { $plan.path = (Resolve-Path -LiteralPath $From).Path }
    $plan.hash = if ($Sha256) { $Sha256.Trim().ToLowerInvariant() } else { $null }
    Write-Host ('[plan]    source = {0}' -f $(if ($plan.url) { 'url' } else { 'local file' }))
    Write-Host ('[plan]    {0}' -f $(if ($plan.url) { $plan.url } else { $plan.path }))
    if ($Check) {
        Write-Host '[check]   -From is an explicit source: there is no "newer version" question to answer; nothing was downloaded'
        exit $EXIT.OK
    }
} else {
    if (-not $currentVersion) {
        Write-Host '[fail] this checkout has no VERSION -- pass -Current <version> so the channel knows what "newer" means'
        exit $EXIT.PREREQ
    }
    $probe = Join-Path $packageDir 'scripts\probe-release-channel.mjs'
    if (-not (Test-Path -LiteralPath $probe)) {
        Write-Host ('[fail] the channel reader is missing: {0}' -f $probe)
        Write-Host '       (packages carry it from 0.1.3 on; in a source checkout it lives in scripts/)'
        exit $EXIT.PREREQ
    }
    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) {
        Write-Host '[fail] Node.js is not on PATH -- the channel reader and home.js both need it'
        Write-Host '       manual path: tray -> Quit, copy the home, unpack the new zip elsewhere, double-click its exe'
        exit $EXIT.PREREQ
    }

    $probeArgs = @($probe, '--channel', $Channel, '--json')
    if ($Current) { $probeArgs += @('--current', $Current) } else { $probeArgs += @('--package', $packageDir) }
    if ($Repo) { $probeArgs += @('--repo', $Repo) }

    Push-Location -LiteralPath $packageDir
    try {
        $probeOut = @(& node @probeArgs 2>&1 | ForEach-Object { $_.ToString() })
        $probeCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $probeOut | ForEach-Object { Write-Host $_ }
    if ($probeCode -ne 0) {
        $label = switch ($probeCode) {
            2 { 'the channel cannot answer (no release yet, or not reachable) -- nothing was changed' }
            3 { 'the channel answered in a shape this build cannot read -- nothing was changed' }
            4 { 'usage: the channel reader wants --current/--package' }
            default { ('the channel reader exited {0}' -f $probeCode) }
        }
        Write-Host ('[fail] {0}' -f $label)
        if ($probeCode -eq 4) { exit $EXIT.USAGE }
        exit $EXIT.CHANNEL
    }
    $jsonLine = @($probeOut | Where-Object { $_ -match '^\{' } | Select-Object -Last 1)
    if ($jsonLine.Count -eq 0) {
        Write-Host '[fail] the channel reader printed no JSON line -- refuse to guess'
        exit $EXIT.CHANNEL
    }
    $readout = $jsonLine[0] | ConvertFrom-Json
    $plan.latest = $readout.latest
    $plan.hasUpdate = $readout.hasUpdate
    $plan.url = $readout.answer.entry.asset.url
    $plan.hash = if ($readout.answer.entry.hash) { ([string]$readout.answer.entry.hash).ToLowerInvariant() } else { $null }
    if ($Sha256) { $plan.hash = $Sha256.Trim().ToLowerInvariant() }

    if ($plan.hasUpdate -eq $false) {
        Write-Host ('[update] already latest ({0}) -- nothing to do' -f $currentVersion)
        exit $EXIT.OK
    }
    if (-not $plan.url) {
        Write-Host '[fail] the channel answered a version but no asset url -- refuse to guess a download address'
        exit $EXIT.CHANNEL
    }
}

# ---------------------------------------------------------------------------
# 3) the hash policy (fail closed) and -Check / -DryRun
# ---------------------------------------------------------------------------
$policy = Test-HashPolicy -Given $plan.hash -UnverifiedAllowed:$AllowUnverified
if (-not $policy.ok) {
    Write-Host ('[fail] {0}' -f $policy.why)
    Write-Host '       the hash is the only proof that these bytes are that version; a version number alone is a claim'
    exit $EXIT.NEEDS_YOU
}
if ($policy.why -eq 'unverified-allowed') {
    Write-Host '[warn] no hash: this run CANNOT prove the bytes (said out loud because -AllowUnverified was given)'
}

if ($Check) {
    Write-Host ('[check]   newer package available: {0} -> {1}' -f $currentVersion, $plan.latest)
    Write-Host ('[check]   asset = {0}' -f $plan.url)
    if ($plan.hash) { Write-Host ('[check]   sha256 = {0}' -f $plan.hash) } else { Write-Host '[check]   sha256 = (channel gave none)' }
    Write-Host '[check]   nothing was downloaded, stopped or unpacked'
    exit $EXIT.OK
}

if ($plan.hash -and $plan.hash -notmatch '^[0-9a-f]{64}$') {
    Write-Host ('[fail] -Sha256 must be 64 hex characters, got {0}' -f $plan.hash)
    exit $EXIT.USAGE
}

# ---------------------------------------------------------------------------
# 4) the shell must be gone (Windows cannot replace a running exe)
# ---------------------------------------------------------------------------
$appProcesses = @(Get-Process -Name 'celira-app' -ErrorAction SilentlyContinue)
if ($appProcesses.Count -gt 0) {
    Write-Host ('[fail] celira-app is still running (pid {0}) -- tray icon -> Quit, then run this again' -f (($appProcesses | ForEach-Object { $_.Id }) -join ', '))
    Write-Host '       closing the window only hides it: the services keep running, and the swap would race them'
    exit $EXIT.NEEDS_YOU
}

if ($DryRun) {
    Write-Host ''
    Write-Host '[dry-run] would do, in this order:'
    Write-Host ('          1. download {0}' -f $plan.url)
    Write-Host ('          2. check sha256 {0}' -f $(if ($plan.hash) { $plan.hash } else { '(none: unverified)' }))
    Write-Host  '          3. node home.js down      (stop this home''s world and residents)'
    Write-Host  '          4. node home.js backup    (BEFORE the new version boots: migrations only go up)'
    Write-Host ('          5. unpack into a NEW folder next to {0}' -f $packageDir)
    Write-Host  '          6. stop here: close the old app and double-click the new exe (the home is untouched)'
    exit $EXIT.OK
}

# ---------------------------------------------------------------------------
# 5) download (and prove the bytes) before anything is stopped
# ---------------------------------------------------------------------------
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$downloadDir = Join-Path ([System.IO.Path]::GetTempPath()) ('celira-update-' + $stamp)
New-Item -ItemType Directory -Force -Path $downloadDir | Out-Null
# Keep the asset's own file name: whoever looks at this download later should recognize it as the
# release asset (the hash on the release page is quoted per asset name).
$zipName = if ($plan.url) { [System.IO.Path]::GetFileName(([uri]$plan.url).AbsolutePath) } else { Split-Path -Leaf $plan.path }
if (-not $zipName -or $zipName -notmatch '\.zip$') { $zipName = 'celira-package-' + $stamp + '.zip' }
$zipPath = Join-Path $downloadDir $zipName

if ($plan.url) {
    Write-Host ('[download] {0}' -f $plan.url)
    $ProgressPreference = 'SilentlyContinue'
    # Two routes, same reason as the channel reader: Windows PowerShell 5.1's Invoke-WebRequest goes
    # through the SYSTEM proxy (WinINET) and, behind a proxy, fails with "Received an unexpected EOF
    # or 0 bytes from the transport stream" on this machine (measured 2026-10-21). curl honours the
    # HTTP(S)_PROXY env vars, streams big files, and printed 200 with the same URL. So: curl first
    # when it exists, Invoke-WebRequest with an explicit proxy as the fallback (and TLS 1.2 forced --
    # old .NET defaults negotiate something the proxy will not carry).
    $proxyUrl = $null
    foreach ($name in 'HTTPS_PROXY', 'https_proxy', 'ALL_PROXY', 'all_proxy', 'HTTP_PROXY', 'http_proxy') {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value -and $value.Trim() -ne '') { $proxyUrl = $value.Trim(); break }
    }
    if ($proxyUrl) { Write-Host ('[download] proxy in the environment: {0}' -f $proxyUrl) }

    $curl = Get-Command 'curl.exe' -ErrorAction SilentlyContinue
    if (-not $curl) { $curl = Get-Command 'curl' -ErrorAction SilentlyContinue }
    if ($curl) {
        Write-Host '[download] via curl'
        & $curl.Source @('-sS', '-L', '--max-time', '900', '-A', 'celira-update/1', '-o', $zipPath, $plan.url) | Out-Host
        $downloadCode = $LASTEXITCODE
    } else {
        Write-Host '[download] via Invoke-WebRequest (no curl on PATH)'
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        } catch {
            /* older runtimes: leave the default alone */
        }
        try {
            if ($proxyUrl) {
                Invoke-WebRequest -Uri $plan.url -OutFile $zipPath -UseBasicParsing -Proxy $proxyUrl
            } else {
                Invoke-WebRequest -Uri $plan.url -OutFile $zipPath -UseBasicParsing
            }
            $downloadCode = 0
        } catch {
            Write-Host ('[download] Invoke-WebRequest: {0}' -f $_.Exception.Message)
            $downloadCode = 1
        }
    }
    if ($downloadCode -ne 0) {
        Write-Host ('[fail] download failed (exit {0}) -- nothing was stopped, nothing was changed' -f $downloadCode)
        exit $EXIT.CHANNEL
    }
} else {
    Write-Host ('[download] local file: {0}' -f $plan.path)
    Copy-Item -LiteralPath $plan.path -Destination $zipPath
}

if (-not (Test-Path -LiteralPath $zipPath)) {
    Write-Host '[fail] the package never landed on disk -- nothing was changed'
    exit $EXIT.CHANNEL
}
Write-Host ('[download] {0} bytes' -f (Get-Item -LiteralPath $zipPath).Length)

if ($plan.hash) {
    $actual = Get-HashOf $zipPath
    if ($actual -ne $plan.hash) {
        Write-Host ('[fail] sha256 mismatch -- refusing to go on')
        Write-Host ('       expected {0}' -f $plan.hash)
        Write-Host ('       actual   {0}' -f $actual)
        Write-Host ('[fail] nothing was stopped, nothing was changed; the download is at {0}' -f $zipPath)
        exit $EXIT.CHANNEL
    }
    Write-Host ('[download] sha256 ok: {0}' -f $actual)
} else {
    Write-Host '[warn] no hash was available: these bytes are NOT proven (recorded here so nobody has to guess later)'
}

# ---------------------------------------------------------------------------
# 6) stop the home, then back it up (while nothing is running)
# ---------------------------------------------------------------------------
$homeDir = Resolve-HomeDir -Explicit $HomeDir
if ($homeDir) { Write-Host ('[home]    {0}' -f $homeDir) }
else { Write-Host '[home]    (the platform default -- home.js will say which one)' }
if ($homeDir) { $env:CELIRA_HOME_DIR = $homeDir }

Write-Host '[stop]    node home.js down'
$downCode = Invoke-NodeIn -Where $packageDir -Arguments @('home.js', 'down')
if ($downCode -ne 0) {
    Write-Host ('[fail] home.js down exited {0} -- stop here: the backup and the swap need a clean stop' -f $downCode)
    Write-Host '[fail] nothing was replaced; the downloaded package is at ' -NoNewline
    Write-Host $zipPath
    exit $EXIT.NEEDS_YOU
}

Write-Host '[backup]  node home.js backup'
$backupCode = Invoke-NodeIn -Where $packageDir -Arguments @('home.js', 'backup')
if ($backupCode -ne 0) {
    Write-Host ('[fail] home.js backup exited {0} -- refusing to install a version whose way back is unproven' -f $backupCode)
    Write-Host '       (backup refuses while anything is running, or when the last stop was not clean)'
    Write-Host ('[fail] nothing was replaced; the downloaded package is at {0}' -f $zipPath)
    exit $EXIT.NEEDS_YOU
}

# ---------------------------------------------------------------------------
# 7) unpack into a NEW directory next to the one being replaced
# ---------------------------------------------------------------------------
$updateDir = New-UpdateDirectory -Parent (Split-Path -Parent $packageDir) -Name 'celira-update' -Stamp $stamp
Write-Host ('[unpack]  {0} -> {1}' -f (Split-Path -Leaf $zipPath), $updateDir)
New-Item -ItemType Directory -Force -Path $updateDir | Out-Null
try {
    Expand-Archive -LiteralPath $zipPath -DestinationPath $updateDir -Force
} catch {
    Write-Host ('[fail] unpack failed: {0}' -f $_.Exception.Message)
    Write-Host ('[fail] the home was stopped and backed up; the old package is untouched at {0}' -f $packageDir)
    exit $EXIT.UNPACK
}

# The zip may have been made from inside the package folder (entries at the root) or from its parent
# (one top-level folder). Both shapes are real; find the directory that actually holds the exe.
$exeName = 'celira-app.exe'
$newRoot = $updateDir
if (-not (Test-Path -LiteralPath (Join-Path $newRoot $exeName))) {
    $candidates = @(Get-ChildItem -LiteralPath $updateDir -Directory | Where-Object {
        Test-Path -LiteralPath (Join-Path $_.FullName $exeName)
    })
    if ($candidates.Count -ge 1) { $newRoot = $candidates[0].FullName }
}
if (-not (Test-Path -LiteralPath (Join-Path $newRoot $exeName))) {
    Write-Host ('[fail] {0} is not in the unpacked package -- that zip is not an all-in-one build' -f $exeName)
    Write-Host ('[fail] the home was stopped and backed up; the old package is untouched at {0}' -f $packageDir)
    exit $EXIT.UNPACK
}
$newVersion = Read-Version (Join-Path $newRoot 'VERSION')
if (-not $newVersion) {
    Write-Host '[warn] the unpacked package has no VERSION -- it will not say which version it is'
} elseif ($plan.latest -and $newVersion -ne $plan.latest) {
    Write-Host ('[warn] the channel said {0} but the unpacked package says {1}' -f $plan.latest, $newVersion)
}
Write-Host ('[unpack]  ready: {0} (version {1})' -f $newRoot, $(if ($newVersion) { $newVersion } else { 'unknown' }))

# ---------------------------------------------------------------------------
# 8) hand over (this script does not start the new build, and does not delete the old one)
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '[next]    double-click ' -NoNewline
Write-Host (Join-Path $newRoot $exeName)
Write-Host ('[next]    the old folder stays put as the way back: {0}' -f $packageDir)
Write-Host ('[next]    the home was stopped, not changed: starting the new build opens the same world')
Write-Host '[next]    rollback (only if the new build already booted and wrote a migration):'
Write-Host '[next]      node home.js down   (from the NEW folder)'
Write-Host '[next]      restore the home from the backup printed above, then start the old folder'
Write-Host ('[keep]    downloaded package kept at {0}' -f $zipPath)
exit $EXIT.OK
