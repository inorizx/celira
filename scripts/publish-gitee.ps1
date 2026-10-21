#Requires -Version 5.1
<#
    Publish ONE release to Gitee, with the SAME bytes that were just built.

        $env:GITEE_TOKEN = '<token with projects scope>'
        .\scripts\publish-gitee.ps1 -Tag v0.1.3 -Zip dist-release\celira-0.1.3-win-x64.zip `
                                    -Repo myloveinori/celira -NotesFile dist-release\RELEASE_NOTES.md

        .\scripts\publish-gitee.ps1 -Check ...      # ask only: does this release exist?
        .\scripts\publish-gitee.ps1 -DryRun ...     # print the two calls, change nothing
        .\scripts\publish-gitee.ps1 -SelfTest       # local assertions, no network, no token

    Why this exists: make_release.ps1 used to stop at "Gitee needs its own OpenAPI, not
    implemented". That was honest, but it left the second channel to a human clicking through
    a web form - and a release that needs two manual uploads is a release where the two channels
    drift. This script is the missing second hand; make_release.ps1 calls it.

    The contract with the other channel (the one that makes a mirrored release a MIRROR):
      - the tag, the file name, the byte count and the sha256 are computed ONCE, by the caller;
      - this script refuses to upload a zip whose sha256 it cannot read;
      - the sha256 is written into the release body as a `sha256: <64 hex>` line, because the
        Gitee API does not return an asset digest. A channel that cannot answer "which bytes"
        is not a release channel for an updater that fails closed.
      - after upload it reads the release back and compares the asset size. `-VerifyDownload`
        additionally downloads the asset and compares the digest (slower, 30 MiB).

    Exit codes: 0 = published (or already there, same size), 1 = usage, 2 = prerequisite
                (no token / no zip / no notes), 4 = Gitee API refused, 5 = uploaded but the
                read-back does not match.

    ASCII only on purpose: Windows PowerShell 5.1 reads a BOM-less .ps1 with the system code
    page; a non-ASCII character inside a string has already broken one script in this repo.
#>
[CmdletBinding()]
param(
    [string]$Tag,
    [string]$Zip,
    [string]$Repo = '',
    [string]$NotesFile = '',
    [string]$Title = '',
    [string]$TargetCommitish = '',

    # Ask only: does this release exist, and what is attached to it?
    [switch]$Check,

    # Print the plan; no write requests.
    [switch]$DryRun,

    # Download the uploaded asset back and compare its sha256 (30 MiB, extra minute).
    [switch]$VerifyDownload,

    # Run local assertions instead of publishing.
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
$EXIT = @{ OK = 0; USAGE = 1; PREREQ = 2; API = 4; VERIFY = 5 }
$API_ROOT = 'https://gitee.com/api/v5'

# ---------------------------------------------------------------------------
# -1) ASCII only (same rule as make_release.ps1 / update.ps1)
# ---------------------------------------------------------------------------
$selfBytes = [System.IO.File]::ReadAllBytes($PSCommandPath)
for ($i = 0; $i -lt $selfBytes.Length; $i++) {
    if ($selfBytes[$i] -gt 0x7F) {
        Write-Host ('[fail] this script is not ASCII-only (first offending byte at offset {0})' -f $i)
        exit $EXIT.USAGE
    }
}

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

function Get-Sha256 {
    param([string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Format-MiB {
    param([long]$Bytes)
    return ('{0:N1} MiB' -f ($Bytes / 1MB))
}

# Gitee returns asset lists under `assets` in the v5 docs; older responses used `attach_files`.
# Read both, never guess which one is "the" truth.
function Get-Assets {
    param($Release)
    if ($null -eq $Release) { return @() }
    if ($Release.PSObject.Properties.Name -contains 'assets' -and $null -ne $Release.assets) {
        return @($Release.assets)
    }
    if ($Release.PSObject.Properties.Name -contains 'attach_files' -and $null -ne $Release.attach_files) {
        return @($Release.attach_files)
    }
    return @()
}

function Get-AssetUrl {
    param($Asset)
    foreach ($name in 'browser_download_url', 'download_url', 'url') {
        if ($Asset.PSObject.Properties.Name -contains $name) {
            $value = [string]$Asset.$name
            if ($value -and $value.Trim() -ne '') { return $value }
        }
    }
    return $null
}

function Invoke-GiteeGet {
    param([string]$Uri, [string]$Token)
    # The token goes in a header, never in the URL: a URL ends up in error messages and logs.
    $headers = @{ Authorization = ('token ' + $Token) }
    return Invoke-RestMethod -Method Get -Uri $Uri -Headers $headers -TimeoutSec 60
}

function Find-ReleaseByTag {
    param([string]$Repo, [string]$Tag, [string]$Token)
    $uri = '{0}/repos/{1}/releases/tags/{2}' -f $API_ROOT, $Repo, $Tag
    try {
        $release = Invoke-GiteeGet -Uri $uri -Token $Token
        # Gitee answers 200 with the body `null` for a tag that has no release - it does NOT use
        # 404 here. Treating that as "found" made -Check say a release existed with id '' and made
        # the upload path try to attach to nowhere (measured 2026-10-21, first real call).
        # The body arrives as the STRING 'null' (the response is served as text/plain), so both
        # shapes have to be refused - a JSON null and its textual twin are the same answer.
        if ($null -eq $release) { return @{ found = $false; release = $null } }
        if ($release -is [string] -and ($release.Trim() -eq '' -or $release.Trim() -eq 'null')) {
            return @{ found = $false; release = $null }
        }
        if ($release -is [array] -and @($release).Count -eq 0) { return @{ found = $false; release = $null } }
        return @{ found = $true; release = $release }
    } catch {
        $status = $null
        if ($_.Exception.PSObject.Properties.Name -contains 'Response' -and $_.Exception.Response) {
            $status = [int]$_.Exception.Response.StatusCode
        }
        if ($status -eq 404) { return @{ found = $false; release = $null } }
        throw
    }
}

function New-GiteeRelease {
    param([string]$Repo, [string]$Tag, [string]$Token, [string]$Name, [string]$Body, [string]$Target)
    if (-not $Body) { $Body = $Name }
    $uri = '{0}/repos/{1}/releases' -f $API_ROOT, $Repo
    # Form body (not a query string): the token stays out of the URL.
    $form = @{
        access_token      = $Token
        tag_name          = $Tag
        name              = $Name
        body              = $Body
        target_commitish  = $Target
        prerelease        = 'false'
    }
    return Invoke-RestMethod -Method Post -Uri $uri -Body $form -TimeoutSec 120
}

function Add-GiteeAsset {
    param([string]$Repo, [string]$ReleaseId, [string]$Token, [string]$FilePath)

    Add-Type -AssemblyName System.Net.Http | Out-Null
    $client = New-Object System.Net.Http.HttpClient
    $client.Timeout = [TimeSpan]::FromMinutes(30)
    $stream = $null
    $form = $null
    try {
        $form = New-Object System.Net.Http.MultipartFormDataContent
        $form.Add((New-Object System.Net.Http.StringContent($Token)), 'access_token')
        $stream = [System.IO.File]::OpenRead($FilePath)
        $file = New-Object System.Net.Http.StreamContent($stream)
        $form.Add($file, 'file', [System.IO.Path]::GetFileName($FilePath))
        $uri = '{0}/repos/{1}/releases/{2}/attach_files' -f $API_ROOT, $Repo, $ReleaseId
        $response = $client.PostAsync($uri, $form).GetAwaiter().GetResult()
        $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        if (-not $response.IsSuccessStatusCode) {
            return @{ ok = $false; status = [int]$response.StatusCode; body = $text }
        }
        return @{ ok = $true; status = [int]$response.StatusCode; body = $text }
    } finally {
        if ($stream) { $stream.Dispose() }
        if ($form) { $form.Dispose() }
        $client.Dispose()
    }
}

# The digest line that turns a human-readable release note into a machine-readable one.
function Get-DigestLine {
    param([string]$Hash)
    return ('sha256: {0}' -f $Hash)
}

# ---------------------------------------------------------------------------
# 0) -SelfTest: no network, no token, no zip
# ---------------------------------------------------------------------------
if ($SelfTest) {
    $passed = 0
    $failed = 0
    function Assert-That {
        param([string]$What, [bool]$Condition)
        if ($Condition) { $script:passed += 1; Write-Host ('[selftest] ok   ' + $What) }
        else { $script:failed += 1; Write-Host ('[selftest] FAIL ' + $What) }
    }

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('celira-gitee-selftest-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        $probe = Join-Path $tmp 'bytes.bin'
        [System.IO.File]::WriteAllBytes($probe, [byte[]](1, 2, 3, 4, 5))
        $known = Get-Sha256 $probe
        Assert-That 'sha256: 64 lowercase hex' ($known -match '^[0-9a-f]{64}$')
        Assert-That 'sha256: stable' ((Get-Sha256 $probe) -eq $known)
        Set-Content -LiteralPath $probe -Encoding ascii -Value 'different'
        Assert-That 'sha256: changes with the bytes' ((Get-Sha256 $probe) -ne $known)

        Assert-That 'digest line: is exactly what the probe reads' (
            (Get-DigestLine 'deadbeef') -match '^sha256: deadbeef$'
        )

        $asset = [pscustomobject]@{ name = 'x.zip'; size = 12; browser_download_url = 'https://example/x.zip' }
        Assert-That 'asset url: browser_download_url wins' ((Get-AssetUrl $asset) -eq 'https://example/x.zip')
        $asset2 = [pscustomobject]@{ name = 'y.zip'; size = 3; download_url = 'https://example/y.zip' }
        Assert-That 'asset url: falls back to download_url' ((Get-AssetUrl $asset2) -eq 'https://example/y.zip')
        $asset3 = [pscustomobject]@{ name = 'z.zip' }
        Assert-That 'asset url: absent answers $null (never a made-up address)' ($null -eq (Get-AssetUrl $asset3))

        Assert-That 'assets: reads assets' (@(Get-Assets ([pscustomobject]@{ assets = @($asset) })).Count -eq 1)
        Assert-That 'assets: reads attach_files' (@(Get-Assets ([pscustomobject]@{ attach_files = @($asset) })).Count -eq 1)
        Assert-That 'assets: absent answers empty (not a fake one)' (@(Get-Assets ([pscustomobject]@{})).Count -eq 0)

        Assert-That 'format: MiB is human' ((Format-MiB 1048576) -match '^1\.0 MiB$')
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }

    Write-Host ('[selftest] ' + $passed + ' passed, ' + $failed + ' failed')
    if ($failed -gt 0) { exit $EXIT.USAGE }
    if ($passed -le 0) { Write-Host '[selftest] no assertions ran -- that is a failure, not a pass'; exit $EXIT.USAGE }
    exit $EXIT.OK
}

# ---------------------------------------------------------------------------
# 1) inputs
# ---------------------------------------------------------------------------
if (-not $Tag -or $Tag.Trim() -eq '') {
    Write-Host '[fail] -Tag is required (e.g. -Tag v0.1.3)'
    exit $EXIT.USAGE
}
if (-not $Zip -or $Zip.Trim() -eq '') {
    Write-Host '[fail] -Zip is required (the all-in-one package built by make_release.ps1)'
    exit $EXIT.USAGE
}
if (-not $Repo -or $Repo.Trim() -eq '') {
    Write-Host '[fail] -Repo is required (owner/name on Gitee, e.g. myloveinori/celira)'
    exit $EXIT.USAGE
}
if ($Repo -notmatch '^[^/\s]+/[^/\s]+$') {
    Write-Host ('[fail] -Repo must be owner/name, got {0}' -f $Repo)
    exit $EXIT.USAGE
}
if (-not (Test-Path -LiteralPath $Zip)) {
    Write-Host ('[fail] no zip at {0}' -f $Zip)
    exit $EXIT.PREREQ
}
$zipPath = (Resolve-Path -LiteralPath $Zip).Path
$zipName = Split-Path -Leaf $zipPath
$zipSize = (Get-Item -LiteralPath $zipPath).Length
$zipHash = Get-Sha256 $zipPath

$notesText = ''
if ($NotesFile -and $NotesFile.Trim() -ne '') {
    if (-not (Test-Path -LiteralPath $NotesFile)) {
        Write-Host ('[fail] no release notes at {0}' -f $NotesFile)
        exit $EXIT.PREREQ
    }
    $notesText = (Get-Content -LiteralPath $NotesFile -Encoding UTF8) -join "`n"
}
if (-not $Title -or $Title.Trim() -eq '') { $Title = ('Celira {0}' -f $Tag) }

# Gitee's create-release call wants a target; the branch this release is cut from is the honest
# default. A package (no git) falls back to the Gitee repository's default branch name, which is
# 'main' for this project - the parameter exists so a different layout can be named explicitly.
if (-not $TargetCommitish -or $TargetCommitish.Trim() -eq '') {
    $branch = (& git rev-parse --abbrev-ref HEAD 2>$null | Select-Object -First 1)
    $TargetCommitish = if ($branch -and $branch.Trim() -ne '' -and $branch.Trim() -ne 'HEAD') { $branch.Trim() } else { 'main' }
}
# The digest must be IN the body: the Gitee API does not report one for an asset, and the updater
# refuses a channel that cannot say which bytes it is offering.
$digest = Get-DigestLine $zipHash
if ($notesText -notmatch '(?im)^\s*sha256\s*:\s*[0-9a-f]{64}\s*$') {
    $notesText = ($notesText.TrimEnd() + "`n`n" + $digest).Trim()
}

Write-Host 'publish-gitee v1'
Write-Host ('repo      = {0}' -f $Repo)
Write-Host ('tag       = {0}' -f $Tag)
Write-Host ('asset     = {0} ({1})' -f $zipName, (Format-MiB $zipSize))
Write-Host ('sha256    = {0}' -f $zipHash)
Write-Host ('title     = {0}' -f $Title)

# ---------------------------------------------------------------------------
# 2) -DryRun: say exactly what would happen
# ---------------------------------------------------------------------------
if ($DryRun) {
    Write-Host ''
    Write-Host '[dry-run] would do, in this order:'
    Write-Host ('          1. GET  {0}/repos/{1}/releases/tags/{2}' -f $API_ROOT, $Repo, $Tag)
    Write-Host '             (found => reuse its id; 404 => create it)'
    Write-Host ('          2. POST {0}/repos/{1}/releases' -f $API_ROOT, $Repo)
    Write-Host ('             tag_name={0} target_commitish={1}' -f $Tag, $TargetCommitish)
    Write-Host ('          3. POST {0}/repos/{1}/releases/<id>/attach_files' -f $API_ROOT, $Repo)
    Write-Host ('             file={0} ({1})' -f $zipName, (Format-MiB $zipSize))
    Write-Host '          4. GET the release back and compare the asset size'
    if ($VerifyDownload) { Write-Host '          5. download the asset back and compare its sha256' }
    Write-Host '[dry-run] nothing was sent to Gitee'
    exit $EXIT.OK
}

# A token is only needed once something will actually be read or written. -DryRun above answers
# without one on purpose: a plan you cannot print before you have a token is a plan nobody checks.
$token = $env:GITEE_TOKEN
if (-not $token -or $token.Trim() -eq '') {
    Write-Host '[fail] GITEE_TOKEN is not set -- this script never asks for a token on the command line'
    Write-Host '       (a token in argv is readable by every process on this machine)'
    Write-Host '       make one at: Gitee -> Settings -> Private Token, scope: projects'
    exit $EXIT.PREREQ
}
$token = $token.Trim()

# ---------------------------------------------------------------------------
# 3) does it exist already?
# ---------------------------------------------------------------------------
$existing = $null
try {
    $found = Find-ReleaseByTag -Repo $Repo -Tag $Tag -Token $token
} catch {
    Write-Host ('[fail] Gitee refused the read: {0}' -f $_.Exception.Message)
    exit $EXIT.API
}

if ($found.found) {
    $existing = $found.release
    Write-Host ('[state]   release {0} already exists (id {1})' -f $Tag, $existing.id)
    $sameName = @(Get-Assets $existing | Where-Object { [string]$_.name -eq $zipName })
    if ($sameName.Count -gt 0) {
        $current = $sameName[0]
        $currentSize = if ($current.PSObject.Properties.Name -contains 'size') { [long]$current.size } else { -1 }
        if ($currentSize -eq $zipSize) {
            Write-Host ('[state]   {0} is already attached with the same size ({1}) -- nothing to upload' -f $zipName, (Format-MiB $zipSize))
            Write-Host '[state]   (byte identity is the digest printed above; -VerifyDownload proves it by reading it back)'
            if ($Check) { exit $EXIT.OK }
            if ($VerifyDownload) {
                $url = Get-AssetUrl $current
                if (-not $url) {
                    Write-Host '[fail] the release lists the asset but gives no download address -- cannot prove the bytes'
                    exit $EXIT.VERIFY
                }
                $tmpFile = Join-Path ([System.IO.Path]::GetTempPath()) ('celira-gitee-verify-' + [guid]::NewGuid().ToString('N') + '.zip')
                try {
                    Invoke-WebRequest -Uri $url -OutFile $tmpFile -UseBasicParsing
                    $got = Get-Sha256 $tmpFile
                    if ($got -ne $zipHash) {
                        Write-Host ('[fail] downloaded asset sha256 {0} != built {1}' -f $got, $zipHash)
                        exit $EXIT.VERIFY
                    }
                    Write-Host ('[verify]  downloaded asset matches: {0}' -f $got)
                } finally {
                    Remove-Item -Force -LiteralPath $tmpFile -ErrorAction SilentlyContinue
                }
            }
            exit $EXIT.OK
        }
        Write-Host ('[state]   {0} exists with a DIFFERENT size ({1} vs {2}) -- refusing to add a second one' -f $zipName, $currentSize, $zipSize)
        Write-Host '          delete or fix the existing asset first; two bytes for one version is exactly what this guards against'
        exit $EXIT.VERIFY
    }
} else {
    Write-Host ('[state]   release {0} does not exist yet' -f $Tag)
}

if ($Check) {
    Write-Host '[check]   nothing was uploaded'
    exit $EXIT.OK
}

# ---------------------------------------------------------------------------
# 4) create (if needed) + upload
# ---------------------------------------------------------------------------
$releaseId = if ($existing) { $existing.id } else { $null }
if (-not $releaseId) {
    Write-Host '[create]  POST releases'
    try {
        $created = New-GiteeRelease -Repo $Repo -Tag $Tag -Token $token -Name $Title -Body $notesText -Target $TargetCommitish
    } catch {
        Write-Host ('[fail] Gitee refused to create the release: {0}' -f $_.Exception.Message)
        Write-Host '       (check: token scope `projects`; the tag exists on the Gitee repo; the repo is writable)'
        exit $EXIT.API
    }
    $releaseId = $created.id
    if (-not $releaseId) {
        Write-Host '[fail] Gitee answered without a release id -- refusing to guess where to upload'
        exit $EXIT.API
    }
    Write-Host ('[create]  id = {0}' -f $releaseId)
}

Write-Host ('[upload]  {0} ({1})' -f $zipName, (Format-MiB $zipSize))
$upload = Add-GiteeAsset -Repo $Repo -ReleaseId $releaseId -Token $token -FilePath $zipPath
if (-not $upload.ok) {
    Write-Host ('[fail] Gitee refused the upload (HTTP {0})' -f $upload.status)
    Write-Host ('       {0}' -f $upload.body)
    exit $EXIT.API
}
Write-Host ('[upload]  HTTP {0}' -f $upload.status)

# ---------------------------------------------------------------------------
# 5) read it back: after an upload, "it said 201" is a claim, not a fact
# ---------------------------------------------------------------------------
try {
    $after = Find-ReleaseByTag -Repo $Repo -Tag $Tag -Token $token
} catch {
    Write-Host ('[fail] uploaded, but the read-back failed: {0}' -f $_.Exception.Message)
    Write-Host '       the asset may be there; do not call this release published until it is read back'
    exit $EXIT.VERIFY
}
if (-not $after.found) {
    Write-Host '[fail] uploaded, but the release is not readable now -- refusing to call this published'
    exit $EXIT.VERIFY
}
$assets = @(Get-Assets $after.release | Where-Object { [string]$_.name -eq $zipName })
if ($assets.Count -eq 0) {
    Write-Host ('[fail] the release is readable but does not list {0} -- refusing to call this published' -f $zipName)
    exit $EXIT.VERIFY
}
$landed = $assets[0]
$landedSize = if ($landed.PSObject.Properties.Name -contains 'size') { [long]$landed.size } else { -1 }
if ($landedSize -ne $zipSize) {
    Write-Host ('[fail] landed size {0} != built size {1}' -f $landedSize, $zipSize)
    exit $EXIT.VERIFY
}
Write-Host ('[verify]  attached: {0} ({1})' -f $zipName, (Format-MiB $landedSize))

if ($VerifyDownload) {
    $url = Get-AssetUrl $landed
    if (-not $url) {
        Write-Host '[fail] the release lists the asset but gives no download address -- cannot prove the bytes'
        exit $EXIT.VERIFY
    }
    $tmpFile = Join-Path ([System.IO.Path]::GetTempPath()) ('celira-gitee-verify-' + [guid]::NewGuid().ToString('N') + '.zip')
    try {
        Write-Host '[verify]  downloading it back (this is the only proof of the bytes)'
        Invoke-WebRequest -Uri $url -OutFile $tmpFile -UseBasicParsing
        $got = Get-Sha256 $tmpFile
        if ($got -ne $zipHash) {
            Write-Host ('[fail] downloaded asset sha256 {0} != built {1}' -f $got, $zipHash)
            exit $EXIT.VERIFY
        }
        Write-Host ('[verify]  downloaded asset matches: {0}' -f $got)
    } finally {
        Remove-Item -Force -LiteralPath $tmpFile -ErrorAction SilentlyContinue
    }
} else {
    Write-Host '[verify]  size checked; bytes NOT read back (pass -VerifyDownload to prove them)'
}

Write-Host ('[done]    {0} published on {1}' -f $Tag, $Repo)
exit $EXIT.OK
