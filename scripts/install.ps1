#Requires -Version 5.1
<#
    Celira installer (Windows, PowerShell native).

    From a fresh clone to a built and ready-to-boot home, in one command:

        .\scripts\install.ps1

    What it does (each step idempotent, each step prints a reading):

      1. prerequisites : git / cargo / node / npm + Windows version
      2. build         : root workspace (release) -> agent bins (release) -> client (npm)
      3. home          : create the home directory if missing (an existing home is NEVER wiped)
      4. credential    : guide the model API key into the user environment
                         (asked once, never echoed, never logged, never in argv)

    It does NOT: download binaries, install a Rust toolchain, register a service,
    change the home entry scripts, or touch an existing home's contents.

    Exit codes (same reading family as the home scripts):
      0 = ready
      1 = usage error
      2 = a prerequisite is missing or the platform is not supported
      3 = needs you (the model credential was not provided)
      4 = a build step failed

    Notes for maintainers:
      * ASCII only, on purpose. Windows PowerShell 5.1 reads a BOM-less .ps1 with the
        system ANSI code page: non-ASCII comments can swallow line breaks and non-ASCII
        paths show up mangled. A BOM fixes the file case but is risky for the
        `irm <url> | iex` form, so this script stays pure ASCII and derives every path
        from $PSScriptRoot instead of hardcoding one.
      * The credential is read once and written straight to the user environment
        (HKCU\Environment). It never reaches stdout, stderr, a log file or a parameter.
#>
[CmdletBinding()]
param(
    # Where the home lives. Defaults to $env:CELIRA_HOME_DIR, then D:\celira-home
    # (the same default the home scripts use).
    [string]$HomeDir,

    # Use a different variable name for the model credential (testing only).
    [string]$CredentialName = 'DEEPSEEK_API_KEY',

    # Skip the client build (useful when only the Rust side is wanted).
    [switch]$SkipClient,

    # Skip every build step (checks prerequisites and prepares the home only).
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
# Sanity check on the layout: a source clone has Cargo.toml, an unpacked release has bin/.
if (-not ((Test-Path (Join-Path $root 'Cargo.toml')) -or (Test-Path (Join-Path $root 'bin')))) {
    Write-Host '[fail] this script must live in <repo or package>/scripts/ (root not found)'
    exit 1
}

if (-not $HomeDir -or $HomeDir.Trim() -eq '') {
    $HomeDir = $env:CELIRA_HOME_DIR
}
if (-not $HomeDir -or $HomeDir.Trim() -eq '') {
    $HomeDir = 'D:\celira-home'
}

Write-Host 'celira-install v1'
Write-Host ("repo={0}" -f $root)
Write-Host ("home={0}" -f $HomeDir)

# Form: an **unpacked release** (`bin/` and `client-dist/` are both present) or a **source clone**.
# A release package skips the whole build section (the package *is* the artifacts), so its
# prerequisites differ too: no Rust, no Git.
$releaseForm = (Test-Path (Join-Path $root 'bin')) -and (Test-Path (Join-Path $root 'client-dist'))
Write-Host ("form={0}" -f $(if ($releaseForm) { 'release (unpacked)' } else { 'source (clone)' }))
Write-Host ''

# ---------------------------------------------------------------------------
# 1) prerequisites
# ---------------------------------------------------------------------------

$prerequisites = if ($releaseForm) {
    @(
        @{ Name = 'node';   Link = 'https://nodejs.org/en/download';            Why = 'boot the home (node home.js) and serve the page' }
    )
} else {
    @(
        @{ Name = 'git';    Link = 'https://git-scm.com/download/win';           Why = 'clone and version control' },
        @{ Name = 'cargo';  Link = 'https://rustup.rs/';                         Why = 'build the im server and the agent (Rust)' },
        @{ Name = 'node';   Link = 'https://nodejs.org/en/download';             Why = 'build the web client + boot the home (node home.js)' },
        @{ Name = 'npm';    Link = 'https://nodejs.org/en/download';             Why = 'install the client dependencies' }
    )
}

# No Git Bash check any more: the home is booted with `node home.js` (TASK_50), so this
# installer no longer needs a POSIX shell at all. The lookup that used to live here had to
# reject the System32/WindowsApps `bash` stubs (they are the WSL launcher, not Git Bash,
# and probing them starts WSL) - that knowledge is kept in the project notes, not in code.

Write-Host '[deps] checking'
$missing = @()
foreach ($tool in $prerequisites) {
    $found = Get-Command $tool.Name -ErrorAction SilentlyContinue
    if ($found) {
        Write-Host ("       ok    {0}" -f $tool.Name)
    } else {
        Write-Host ("       MISSING {0}  ({1})" -f $tool.Name, $tool.Why)
        Write-Host ("               install: {0}" -f $tool.Link)
        $missing += $tool.Name
    }
}

$osVersion = [System.Environment]::OSVersion.Version
if ($osVersion.Major -lt 10) {
    Write-Host ("[deps] MISSING Windows 10 or newer (found {0})" -f $osVersion)
    # Support surface (same sentence as both READMEs): Windows + Linux, with every v1
    # acceptance run so far on Windows; macOS is outside the surface. No frozen claim here -
    # keep this line in sync with the READMEs when the surface moves.
    Write-Host '       this installer is the Windows path: every v1 acceptance run so far has been on Windows'
    Write-Host '       Linux is the registered extension (no acceptance run there yet); macOS is outside the supported surface'
    $missing += 'windows'
} else {
    Write-Host ("       ok    windows {0}.{1}" -f $osVersion.Major, $osVersion.Minor)
}

if ($missing.Count -gt 0) {
    Write-Host ''
    Write-Host ("[fail] missing prerequisites: {0}" -f ($missing -join ', '))
    Write-Host '       install the ones listed above, then run this script again (nothing was changed)'
    exit 2
}

# ---------------------------------------------------------------------------
# 2) build
# ---------------------------------------------------------------------------

function Invoke-BuildStep {
    param(
        [string]$Title,
        [string]$WorkingDir,
        [string]$Executable,
        [string[]]$Arguments
    )
    Write-Host ("[build] {0}" -f $Title)
    Push-Location $WorkingDir
    try {
        # `| Out-Host` matters: without it the child's stdout lands on THIS function's output
        # pipeline, so the function returns an array (child output + $true/$false) and every
        # `if (-not $built)` silently sees "true" - a failing build would keep going.
        & $Executable @Arguments | Out-Host
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    if ($code -ne 0) {
        Write-Host ("[fail] {0} exited {1}" -f $Title, $code)
        return $false
    }
    return $true
}

if ($releaseForm) {
    # Release form: the artifacts ship inside the package, so the whole build section is skipped
    # - that is what "download and run" means.
    $binaries = @(Get-ChildItem (Join-Path $root 'bin') -Filter *.exe -ErrorAction SilentlyContinue)
    $pageIndex = Join-Path $root 'client-dist\index.html'
    Write-Host ('[build] skipped - unpacked release: {0} executables in bin/, static page {1}' -f `
        $binaries.Count, $(if (Test-Path $pageIndex) { 'present' } else { 'MISSING (client-dist/index.html)' }))
    if ($binaries.Count -eq 0 -or -not (Test-Path $pageIndex)) {
        Write-Host ''
        Write-Host '[fail] this looks like a release package but it is incomplete (see the reading above)'
        exit 4
    }
} elseif ($SkipBuild) {
    Write-Host '[build] skipped (-SkipBuild)'
} else {
    $built = $true
    $built = $built -and (Invoke-BuildStep 'root workspace (release)' $root 'cargo' @('build', '--release'))
    if ($built) {
        $built = $built -and (Invoke-BuildStep 'agent bins (release)' (Join-Path $root 'agent') 'cargo' @('build', '--bins', '--release'))
    }
    if ($built -and (-not $SkipClient)) {
        $client = Join-Path $root 'client'
        $built = $built -and (Invoke-BuildStep 'client dependencies' $client 'npm' @('install'))
        if ($built) {
            $built = $built -and (Invoke-BuildStep 'client bundle' $client 'npm' @('run', 'build'))
        }
    } elseif ($SkipClient) {
        Write-Host '[build] client skipped (-SkipClient)'
    }
    if (-not $built) {
        Write-Host ''
        Write-Host '[fail] a build step failed - fix the message above and run this script again (nothing else was changed)'
        exit 4
    }
}

# ---------------------------------------------------------------------------
# 3) home
# ---------------------------------------------------------------------------

Write-Host '[home] preparing'
$homeExisted = Test-Path $HomeDir
if ($homeExisted) {
    Write-Host ("       exists (kept as is - the home is persistent, this script never wipes it)")
} else {
    Write-Host '       creating'
}
foreach ($sub in @('', 'im\media', 'env', 'materials', 'logs')) {
    $path = if ($sub -eq '') { $HomeDir } else { Join-Path $HomeDir $sub }
    if (-not (Test-Path $path)) {
        New-Item -ItemType Directory -Force -Path $path | Out-Null
    }
}

$manualSource = Join-Path $PSScriptRoot 'HOME_MANUAL.md'
$manualTarget = Join-Path $HomeDir 'README.md'
if ((Test-Path $manualSource) -and (-not (Test-Path $manualTarget))) {
    Copy-Item $manualSource $manualTarget
    Write-Host ("       manual copied: {0}\README.md" -f $HomeDir)
} elseif (Test-Path $manualTarget) {
    Write-Host '       manual already there (home.js copies it from the same source when absent)'
}

# ---------------------------------------------------------------------------
# 4) model credential
# ---------------------------------------------------------------------------

Write-Host '[key] checking the model credential'
$existing = [Environment]::GetEnvironmentVariable($CredentialName, 'User')
if ($existing -and $existing.Trim() -ne '') {
    Write-Host ("       {0} is already set for this user (value never shown)" -f $CredentialName)
} else {
    Write-Host ("       {0} is not set yet." -f $CredentialName)
    Write-Host '       Paste it once below. It is written straight to your user environment'
    Write-Host '       (HKCU\Environment). It is never shown, never logged, never a parameter.'
    Write-Host ('       Leave it empty to stop here and set {0} yourself later.' -f $CredentialName)

    $plain = $null
    if ([Console]::IsInputRedirected) {
        # Non-interactive run (CI or a piped value): read exactly one line from stdin.
        $plain = [Console]::In.ReadLine()
    } else {
        $secure = Read-Host -Prompt '       model API key' -AsSecureString
        if ($secure -and $secure.Length -gt 0) {
            $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
            try {
                $plain = [System.Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
            } finally {
                [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
            }
        }
    }

    if (-not $plain -or $plain.Trim() -eq '') {
        Write-Host ''
        Write-Host ('[fail] needs you: {0} is still not set.' -f $CredentialName)
        Write-Host ('       set it in the Windows user environment, then run: node home.js up')
        $plain = $null
        exit 3
    }
    [Environment]::SetEnvironmentVariable($CredentialName, $plain.Trim(), 'User')
    $plain = $null
    Write-Host ("       {0} written for this user (length not reported either)" -f $CredentialName)
}

# ---------------------------------------------------------------------------
# readings: what is ready, and what to run next
# ---------------------------------------------------------------------------

Write-Host ''
Write-Host '[state]'
Write-Host ("       form      = {0}" -f $(if ($releaseForm) { 'release (unpacked)' } else { 'source (clone)' }))
Write-Host ("       repo      = {0}" -f $root)
Write-Host ("       home      = {0}" -f $HomeDir)
Write-Host ("       home was  = {0}" -f $(if ($homeExisted) { 'already there (kept)' } else { 'created now' }))
if ($releaseForm) {
    Write-Host '       binaries  = from bin/ (shipped with this package)'
} else {
    Write-Host ("       rust      = {0}" -f $(if ($SkipBuild) { 'not built (-SkipBuild)' } else { 'release builds done' }))
}
Write-Host ("       client    = {0}" -f $(if ($releaseForm) { 'static page from client-dist/' } elseif ($SkipBuild -or $SkipClient) { 'not built' } else { 'bundle done (client/dist)' }))
Write-Host ''
Write-Host '[next]'
Write-Host ("       1. boot the home ({0}):" -f $(if ($releaseForm) { 'it starts the executables shipped in bin/' } else { 'it starts the release builds made above' }))
Write-Host '            node home.js up'
Write-Host '       2. open the browser:'
Write-Host '            http://localhost:5173'
Write-Host ''
Write-Host '       (the home is persistent: node home.js down stops it, nothing inside is removed)'

exit 0
