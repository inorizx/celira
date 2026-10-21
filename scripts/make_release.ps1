#Requires -Version 5.1
<#
    Celira release packager (manual, Windows x64).

        .\scripts\make_release.ps1                 # tag defaults to v<Cargo.toml version>
        .\scripts\make_release.ps1 -Tag v0.1.0     # refused if it does not match the version
        .\scripts\make_release.ps1 -SkipPublish    # package + verify locally, never call gh
        .\scripts\make_release.ps1 -SkipBuild      # assemble from the release artifacts already on disk
        .\scripts\make_release.ps1 -Platform linux-x64   # package another platform (build it there first)

    What one run does:
      1. version discipline : the tag must equal "v" + the workspace version (single authority:
                              [workspace.package].version in the root Cargo.toml); the standalone
                              agent crate's own version must agree with it too
      2. build              : root workspace (release) -> agent bins (release) -> client (npm)
      3. assemble           : FOUR packages, one version (see the partition rule below):
                               all-in-one : celira-<version>-win-x64/        (celira-app, bin, client-dist, scripts, docs, VERSION)
                               core       : celira-core-<version>-win-x64/   (the world: im server + the page it serves)
                               home       : celira-home-<version>-win-x64/   (the runtime: agent bins + home.js + the shell)
                               client     : celira-client-<version>-win-x64/ (the page + its server)
      4. zip                : one .zip per package, plus MANIFEST-<part>.txt per package
      5. publish            : one build, one digest, two channels
                                origin github.com  -> gh release create <tag> <zip> <zip>.sha256
                                origin gitee.com   -> scripts/publish-gitee.ps1 (OpenAPI v5)
                                -AlsoGitee <repo>  -> the GitHub path first, then the Gitee mirror
                              both channels upload the SAME bytes and carry the same `sha256:` line
                              in the release body (Gitee's API reports no asset digest, so the body
                              is the machine-readable one)
                              (no gh / no GITEE_TOKEN / no permission => the package is kept and the
                               run reports "not published" with a non-zero exit code; it never
                               pretends. -SkipPublish keeps the old local-only behaviour.)

    The split is a PARTITION check, not a copy list: every file of the all-in-one package must be
    claimed by at least one part, and the only files claimed twice are the declared shared pair
    (client-dist/ + scripts/serve_client.mjs -- the world serves its own page, and the standalone
    client needs that same page and its server). A file nobody claims would silently vanish from
    every download, so that check fails closed.

    WARNING: step 3 DELETES dist-release/celira-<version>-<platform>/ before rebuilding it. If a
    deployment lives there (the machine's `D:\celira-app` once was a junction into exactly that
    directory), one run of this script replaces the thing people are running - and any *.bak copies
    kept there go with it. A deployment belongs in a directory of its own, nowhere near a build
    output: swapping it is a plain "stop -> swap -> start" (the "swap a package" section of
    scripts/HOME_MANUAL.md).

    Exit codes: 0 = packaged and published, 1 = usage error, 2 = version mismatch,
                3 = rebuild refused, 4 = a build step failed, 5 = packaged but not published.

    ASCII only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 with the system code page,
    and this script is run from a repository whose path may contain non-ASCII characters).
#>
[CmdletBinding()]
param(
    [string]$Tag,
    [switch]$SkipPublish,
    [switch]$SkipBuild,
    [string]$Platform = 'win-x64',

    # Publish the SAME bytes to Gitee as well. Empty = do not touch Gitee unless the origin is
    # Gitee itself (then this is filled from the origin). Needs GITEE_TOKEN in the environment.
    [string]$GiteeRepo = '',

    # Origin is GitHub: publish to GitHub with gh, then mirror to -GiteeRepo.
    [switch]$AlsoGitee
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# -1) this file must stay ASCII (its own rule, checked mechanically)
# ---------------------------------------------------------------------------
# The rule is in the header: a BOM-less .ps1 is read with the system ANSI code page, so non-ASCII
# text arrives as mojibake. In a comment that is noise; inside a string it can break the parse and
# the script dies before doing anything (measured 2026-10-12: a Chinese marker in a string made this
# whole packager exit 1 with a mangled syntax error). Checking it here - before the destructive
# assemble step - means the failure names itself instead of looking like a mysterious usage error.
$selfBytes = [System.IO.File]::ReadAllBytes($PSCommandPath)
$nonAsciiAt = -1
for ($i = 0; $i -lt $selfBytes.Length; $i++) {
    if ($selfBytes[$i] -gt 0x7F) { $nonAsciiAt = $i; break }
}
if ($nonAsciiAt -ge 0) {
    Write-Host ('[fail] this script is not ASCII-only (first offending byte at offset {0}) - see the header' -f $nonAsciiAt)
    exit 1
}

# ---------------------------------------------------------------------------
# 0) platform
# ---------------------------------------------------------------------------
# One packager, two platforms. The layout is identical; only the binary suffix, the cargo output
# directory and the VERSION label differ. A platform whose binaries are not on disk fails closed
# (exit 4) with a message that says where to build it - this host cannot cross-compile silently.
$platforms = @{
    'win-x64'   = @{ label = 'windows-x64'; suffix = '.exe'; triple = $null }
    'linux-x64' = @{ label = 'linux-x64';   suffix = '';     triple = 'x86_64-unknown-linux-gnu' }
}
if (-not $platforms.ContainsKey($Platform)) {
    Write-Host ("[fail] unknown -Platform '{0}' (known: {1})" -f $Platform, (($platforms.Keys | Sort-Object) -join ', '))
    exit 1
}
$pf = $platforms[$Platform]
$exeSuffix = $pf.suffix
$platformLabel = $pf.label
$hostIsWindows = ($env:OS -eq 'Windows_NT')
$hostPlatform = if ($hostIsWindows) { 'win-x64' } else { 'linux-x64' }
# Host build -> target/release; cross build -> target/<triple>/release.
$targetLeaf = if ($Platform -eq $hostPlatform) { 'release' } else { Join-Path $pf.triple 'release' }
if (-not $pf.triple -and $Platform -ne $hostPlatform) { $targetLeaf = 'release' }

$root = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path (Join-Path $root 'Cargo.toml'))) {
    Write-Host '[fail] run this from <repo>/scripts/ (repo root not found)'
    exit 1
}

# ---------------------------------------------------------------------------
# 1) version discipline
# ---------------------------------------------------------------------------

function Get-WorkspaceVersion {
    param([string]$Path)
    $inside = $false
    # -Encoding UTF8 is not decoration: the manifests are UTF-8 without a BOM and carry Chinese
    # comments, so Windows PowerShell 5.1's default (ANSI / system code page) decodes them wrong and
    # can swallow line breaks -- `[workspace.package]` then never matches and this script exits 2
    # with "no [workspace.package] version" on a perfectly good manifest (measured 2026-10-21).
    foreach ($line in Get-Content $Path -Encoding UTF8) {
        if ($line -match '^\s*\[workspace\.package\]') { $inside = $true; continue }
        if ($inside -and $line -match '^\s*\[') { break }
        if ($inside -and $line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    return $null
}

function Get-CrateVersion {
    param([string]$Path)
    $inside = $false
    foreach ($line in Get-Content $Path -Encoding UTF8) {
        if ($line -match '^\s*\[package\]') { $inside = $true; continue }
        if ($inside -and $line -match '^\s*\[') { break }
        if ($inside -and $line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    return $null
}

$version = Get-WorkspaceVersion (Join-Path $root 'Cargo.toml')
if (-not $version) {
    Write-Host '[fail] the root Cargo.toml has no [workspace.package] version - that file is the single authority'
    exit 2
}
$agentVersion = Get-CrateVersion (Join-Path $root 'agent\Cargo.toml')
$expectedTag = 'v' + $version
if (-not $Tag -or $Tag.Trim() -eq '') { $Tag = $expectedTag }

# Where this build came from: written into VERSION so an unpacked package can say which release
# channel it belongs to. Until 2026-10-21 the package carried no repo at all, and the updater had
# to ask git - which a downloaded package does not have. The package must be self-describing.
$originUrl = (& git -C $root remote get-url origin 2>$null | Select-Object -First 1)
$originHost = if ($originUrl -match 'gitee\.com') { 'gitee' }
    elseif ($originUrl -match 'github\.com') { 'github' }
    elseif ($originUrl) { 'other' } else { 'none' }
$originSlug = ''
if ($originUrl -and $originUrl -match '(?:github|gitee)\.com[:/]([^/]+)/([^/]+?)(?:\.git)?$') {
    $originSlug = '{0}/{1}' -f $Matches[1], $Matches[2]
}
if (-not $GiteeRepo -and $originHost -eq 'gitee') { $GiteeRepo = $originSlug }

Write-Host 'celira-release v1'
Write-Host ("version   = {0}   (root Cargo.toml: [workspace.package].version)" -f $version)
Write-Host ("platform  = {0}   (this host is {1})" -f $Platform, $hostPlatform)
Write-Host ("agent     = {0}   (agent/ is a standalone crate; its version must agree)" -f $agentVersion)
Write-Host ("tag       = {0}" -f $Tag)

if ($Tag -ne $expectedTag) {
    Write-Host ("[fail] tag/version mismatch: the tag is {0} but the workspace version is {1} (expected tag {2})" -f $Tag, $version, $expectedTag)
    exit 2
}
if ($agentVersion -ne $version) {
    Write-Host ("[fail] agent/ says {0} while the workspace says {1} - one release carries one version" -f $agentVersion, $version)
    exit 2
}

$rev = (& git -C $root rev-parse --short HEAD 2>$null)
if ($LASTEXITCODE -ne 0) { $rev = 'unknown' }
$dirty = (& git -C $root status --porcelain 2>$null)
# The rev goes into VERSION, and VERSION is what names a release directory in the release-root
# layout (`<root>\releases\<version>+<git rev>`). A build from a dirty tree therefore used to claim
# the clean revision - two different contents under one identity. The suffix keeps them apart, and
# says out loud which one this is.
if ($dirty -and $rev -ne 'unknown') { $rev = '{0}-dirty' -f $rev }
Write-Host ("rev       = {0}{1}" -f $rev, $(if ($dirty) { ' (working tree has uncommitted changes)' } else { '' }))
Write-Host ''

# ---------------------------------------------------------------------------
# 2) build
# ---------------------------------------------------------------------------

function Invoke-BuildStep {
    param([string]$Title, [string]$WorkingDir, [string]$Executable, [string[]]$Arguments)
    Write-Host ("[build] {0}" -f $Title)
    Push-Location $WorkingDir
    try {
        # `| Out-Host` matters: without it the child's stdout lands on THIS function's output
        # pipeline, so the function returns an array (child output + $true/$false) and every
        # `if (-not $built)` silently sees "true" - a failing build would package a stale bundle.
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

if ($SkipBuild) {
    Write-Host '[build] skipped (-SkipBuild): reusing what is already in target/release and client/dist'
} else {
    $cargoTarget = @()
    if ($Platform -ne $hostPlatform -and $pf.triple) { $cargoTarget = @('--target', $pf.triple) }
    $built = Invoke-BuildStep 'root workspace (release)' $root 'cargo' (@('build', '--release') + $cargoTarget)
    if ($built) { $built = Invoke-BuildStep 'agent bins (release)' (Join-Path $root 'agent') 'cargo' (@('build', '--bins', '--release') + $cargoTarget) }
    # `npm.cmd` on purpose (not `npm`): Node's npm.ps1 shim, when invoked from inside a
    # script, rebuilds its arguments by re-parsing the *calling source line* instead of
    # using the actual arguments -- with `& $Executable @Arguments` the variable names
    # reach npm as literal words (`Unknown command: "Executable"`, found live in the
    # TASK_51 close-out run on PS 5.1). npm.cmd receives the real argv; cargo is a
    # plain .exe, unaffected. (ASCII only: same discipline as install.ps1.)
    # Platform mapping: Linux ships no `npm.cmd` at all (invoking it there throws
    # CommandNotFoundException and pwsh exits 1 -- the script's own exit-4 branch never
    # runs), and the npm.ps1 re-parse trap above is Windows-specific anyway. Windows keeps
    # `npm.cmd` unchanged; every other host calls plain `npm`.
    $npmExe = if ($hostIsWindows) { 'npm.cmd' } else { 'npm' }
    if ($built) { $built = Invoke-BuildStep 'client bundle' (Join-Path $root 'client') $npmExe @('run', 'build') }
    # The desktop shell (the product form a human double-clicks). `tauri/custom-protocol` is what
    # makes it embed client/dist instead of pointing at a dev server: without that feature the
    # binary carries the devUrl and opens a blank/refused page (found live 2026-10-05).
    if ($built) { $built = Invoke-BuildStep 'desktop shell (release, embedded page)' (Join-Path $root 'app\src-tauri') 'cargo' (@('build', '--release', '--features', 'tauri/custom-protocol') + $cargoTarget) }
    if (-not $built) {
        Write-Host '[fail] a build step failed - nothing was packaged'
        exit 4
    }
}

# ---------------------------------------------------------------------------
# 3) assemble
# ---------------------------------------------------------------------------

$name = 'celira-{0}-{1}' -f $version, $Platform
$releaseRoot = Join-Path $root 'dist-release'
$packageDir = Join-Path $releaseRoot $name
# Every executable the home scripts or the manual can invoke, plus the im server.
# `environment_worker` and the `*_fixture*` binaries are test-only helpers and are never shipped.
# Do not trim this list by eyeballing it: the check after the copy fails closed when the manual
# promises one of our executables that is not in bin/ (that is how `agent_host_acknowledge` was
# found missing - the manual and `agent_host_status`'s [next] line both point at it).
$binaries = @(
    (Join-Path $root ('target\{0}\celira-im{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_host{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_host_status{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_host_acknowledge{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_run{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\environment_diagnose{1}' -f $targetLeaf, $exeSuffix))
)

# Validate every input BEFORE creating anything: a failed run must not leave a half-built directory
# behind. An empty celira-<version>-<platform>/ looks exactly like a package to every later glob -
# found live when a linux-x64 request failed on this Windows host and left one sitting there.
foreach ($binary in $binaries) {
    if (-not (Test-Path $binary)) {
        Write-Host ("[fail] expected build output is missing: {0}" -f $binary)
        if ($Platform -ne $hostPlatform) {
            Write-Host ("       this host is {0} and the request is {1}: build those binaries ON {1} (or install the rust target + a cross linker), then re-run with -SkipBuild" -f $hostPlatform, $Platform)
        }
        exit 4
    }
}

# The desktop shell ships at the PACKAGE ROOT, not in bin/: the shell finds its runtime by walking up
# from its own executable looking for a directory that has bin/agent_host.exe (home_facts.rs
# `resolve_repo_dir` + `detect_layout`'s packaged branch). Root shell + bin/ runtime is exactly that
# shape, so `celira-app.exe` runs from an unpacked package on a machine with no repository at all
# (verified 2026-10-05 by running it from a repo-less directory: it started the world out of its own
# bin/). The 2026-10-05 ruling: the all-in-one package is what an ordinary user gets.
$appBinary = Join-Path $root ('app\src-tauri\target\{0}\celira-app{1}' -f $targetLeaf, $exeSuffix)
if (-not (Test-Path $appBinary)) {
    Write-Host ("[fail] expected build output is missing: {0}" -f $appBinary)
    Write-Host '       build it with: cargo build --release --features tauri/custom-protocol (in app/src-tauri)'
    exit 4
}

if (Test-Path $packageDir) {
    # Say it before it happens: this line deletes a whole directory, and a deployment used to live
    # in exactly this one (the machine's `D:\celira-app` was a junction into it) - so one packaging
    # run silently replaced the thing people were running. The release-root layout ends that
    # (the machine's live copy used to live in exactly this directory); this readout exists so that
# nobody has to remember it.
    Write-Host ("[assemble] deleting and rebuilding: {0}" -f $packageDir)
    Write-Host '           (a deployment must NOT live here: give it a directory of its own)'
    Remove-Item -Recurse -Force $packageDir
}
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir 'bin'), (Join-Path $packageDir 'client-dist'), (Join-Path $packageDir 'scripts'), (Join-Path $packageDir 'docs') | Out-Null

# Every executable the home scripts or the manual can invoke, plus the im server.
# `environment_worker` and the `*_fixture*` binaries are test-only helpers and are never shipped.
# Do not trim this list by eyeballing it: the check after the copy fails closed when the manual
# promises one of our executables that is not in bin/ (that is how `agent_host_acknowledge` was
# found missing - the manual and `agent_host_status`'s [next] line both point at it).
$binaries = @(
    (Join-Path $root ('target\{0}\celira-im{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_host{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_host_status{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_host_acknowledge{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\agent_run{1}' -f $targetLeaf, $exeSuffix)),
    (Join-Path $root ('agent\target\{0}\environment_diagnose{1}' -f $targetLeaf, $exeSuffix))
)
foreach ($binary in $binaries) {
    if (-not (Test-Path $binary)) {
        Write-Host ("[fail] expected build output is missing: {0}" -f $binary)
        if ($Platform -ne $hostPlatform) {
            Write-Host ("       this host is {0} and the request is {1}: build those binaries ON {1} (or install the rust target + a cross linker), then re-run with -SkipBuild" -f $hostPlatform, $Platform)
        }
        exit 4
    }
    Copy-Item $binary (Join-Path $packageDir 'bin')
}
Copy-Item $appBinary (Join-Path $packageDir ('celira-app' + $exeSuffix))
Copy-Item (Join-Path $root 'client\dist\*') (Join-Path $packageDir 'client-dist') -Recurse
# The manual is copied twice, on purpose - it has two consumers that look in different places:
#   * scripts/HOME_MANUAL.md - what the home scripts and install.ps1 copy into <home>/README.md;
#   * docs/HOME_MANUAL.md    - where a human browsing the unpacked package looks for it.
# Shipping only the docs/ copy is a silent failure: both readers test for the scripts/ path first
# and simply skip their "manual copied" step, so the manual never reaches the home at all.
# install.ps1 is the Windows installer; on other platforms it is NOT shipped (there is no
# install.sh yet - registered in the split report, not papered over by shipping a wrong-OS script).
$scriptFiles = @('serve_client.mjs', 'HOME_MANUAL.md', 'probe-release-channel.mjs')
# update.ps1 (2026-10-21) is the Windows side of "does a newer package exist, and how does it land":
# its first step asks the channel reader, so the two ship together -- shipping the updater without the
# reader would be shipping a script that always stops at step 1.
if ($Platform -eq 'win-x64') { $scriptFiles = @('install.ps1', 'update.ps1') + $scriptFiles }
foreach ($script in $scriptFiles) {
    Copy-Item (Join-Path $PSScriptRoot $script) (Join-Path $packageDir 'scripts')
}
# The upgrade tooling used to be copied here (scripts/upgrade/*.ps1) so that the manual could teach
# it. Both are gone on purpose: swapping a deployment is a plain "stop -> swap -> start" with the
# package itself as the way back, so there is nothing to ship - see the "swap a package" section of
# scripts/HOME_MANUAL.md.
Copy-Item (Join-Path $PSScriptRoot 'HOME_MANUAL.md') (Join-Path $packageDir 'docs\HOME_MANUAL.md')
# The desktop walkthrough: the all-in-one is what an ordinary user gets (2026-10-05 ruling), and the
# four steps below are the ones actually walked on 2026-10-05 against this very package layout
# (unzip -> celira-app.exe -> create an agent -> open a room -> send; she answered in seconds).
# It ships at the package ROOT next to celira-app.exe, not only under docs/: the person who just
# unzipped is looking at the exe, and a manual they have to go find is a manual they will not read.
Copy-Item (Join-Path $PSScriptRoot 'APP_MANUAL.md') (Join-Path $packageDir 'APP_MANUAL.md')
Copy-Item (Join-Path $PSScriptRoot 'APP_MANUAL.md') (Join-Path $packageDir 'docs\APP_MANUAL.md')

# The core's own deployment doc (the world: how to run it, how to open the first identity, what to
# change in the cloud). Before this line the core package shipped with no instructions at all.
Copy-Item (Join-Path $root 'im\server\README.md') (Join-Path $packageDir 'docs\CORE_README.md')

# home.js (repo root) is the home entry point the manual tells the user to run: `node home.js up`.
# It replaces the .sh scripts above and needs no Git Bash, so it ships at the package root with its
# modules under scripts/home/ - the exact layout the repo has, so `node home.js up` works from an
# unpacked package just like from a clone (relative requires resolve inside the package).
Copy-Item (Join-Path $root 'home.js') (Join-Path $packageDir 'home.js')
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir 'scripts\home') | Out-Null
# Every FILE under scripts/home/ ships, not just *.js: model-windows.js reads model-windows.json
# next to itself (path.join(__dirname, 'model-windows.json')) and silently falls back to an empty
# table when that file is missing, so a *.js-only copy shipped a release whose model-config page
# quietly lost its context-window defaults. The set-equality check below keeps that from returning.
Get-ChildItem (Join-Path $PSScriptRoot 'home') -File | Copy-Item -Destination (Join-Path $packageDir 'scripts\home')

$builtAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
# The note is read by whoever unpacked the zip. SmartScreen is a Windows thing; the other
# platforms get the same sentence without it (the Windows line stays byte-identical).
# Since 2026-10-05 the all-in-one carries the desktop shell, so "Node.js is required" is no longer
# the first thing an ordinary user needs to know: double-clicking celira-app.exe needs no Node at all
# (the page is embedded in the binary). Node is only for the CLI form (`node home.js up`).
$versionNote = if ($Platform -eq 'win-x64') {
    'note    = unsigned binaries (SmartScreen may warn); ordinary users: run celira-app.exe (no Node.js needed); CLI form needs Node.js (node home.js up)'
} else {
    'note    = unsigned binaries; ordinary users: run celira-app (no Node.js needed); CLI form needs Node.js (node home.js up)'
}
# The release channel this build belongs to. `repo` is where it is published first; `mirror` is
# the second copy of the SAME bytes. The updater reads these lines instead of asking git (a
# downloaded package has no .git). Empty values are omitted, never written as a blank key.
$channelRows = @()
if ($originSlug) { $channelRows += ('repo    = {0}' -f $originSlug) }
if ($GiteeRepo -and $GiteeRepo -ne $originSlug) { $channelRows += ('mirror  = {0}' -f $GiteeRepo) }
Set-Content -Path (Join-Path $packageDir 'VERSION') -Encoding ascii -Value (@(
    ("version = {0}" -f $version),
    ("git rev = {0}" -f $rev),
    ("built   = {0}" -f $builtAt),
    ("platform = {0}" -f $platformLabel),
    'package = all-in-one'
) + $channelRows + @($versionNote))

# Package check: code artifacts only (no credentials, no home content, no build cache)
$leaked = @(Get-ChildItem -Recurse -File $packageDir | Where-Object {
    $_.Name -match '^(home\.conf|identity\.json|facts\.ndjson|trace\.ndjson)$' -or $_.Name -like '.env*'
})
if ($leaked.Count -gt 0) {
    Write-Host ('[fail] the package would contain credential-like files: ' + (($leaked | ForEach-Object { $_.FullName }) -join '; '))
    exit 3
}

# Package check: the manual ships WITH the package, so every one of our executables it tells the
# user to run must actually be in this package. This is the "download and run" promise, and it is
# mechanical - the manual is the spec, the package is the artifact, and neither can drift quietly.
# Only our own family is matched: the MCP section shows a third-party path (`.../server.exe`) as
# an example, and that one is not ours to ship.
# The manual is ONE file for every platform and spells our executables the Windows way (.exe);
# a promise must be met by THIS platform's file name, so the promised names are mapped onto the
# platform's suffix before comparing (on Windows the mapping is the identity - same check, same
# message, byte for byte).
#
# Where a promised executable may live: bin/, or the PACKAGE ROOT. The second half was added
# 2026-10-12, when the manual started naming `celira-app.exe` in the upgrade sections: the desktop
# shell ships at the root on purpose (it finds its runtime by walking up from its own executable -
# see the note at the copy step), so "in bin/" was never the promise - "in this package" is. The
# readout names the ones found at the root, so nothing is silently excused.
# Two manuals ship and they have two readers: docs/HOME_MANUAL.md (the CLI / operator form) and
# APP_MANUAL.md (somebody who just unpacks and double-clicks). The promise is the same for both -
# whatever a shipped doc tells a reader to run has to BE in the package - so one check reads both.
# It fails just as hard for the ordinary reader ("file not found") as for an operator.
$manualPaths = @((Join-Path $packageDir 'docs\HOME_MANUAL.md'), (Join-Path $packageDir 'APP_MANUAL.md'))
foreach ($manualPath in $manualPaths) {
    if (-not (Test-Path -LiteralPath $manualPath)) {
        Write-Host ('[fail] a shipped manual is missing from the package: ' + (Split-Path -Leaf $manualPath))
        exit 4
    }
}
$manualText = (@($manualPaths | ForEach-Object { Get-Content -Raw $_ }) -join "`n")
$shipped = @(Get-ChildItem (Join-Path $packageDir 'bin') -Filter ('*' + $exeSuffix) | ForEach-Object { $_.Name })
$promised = @([regex]::Matches($manualText, '((?:agent|environment|celira)[A-Za-z0-9_\-]*\.exe)') |
    ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
$expectedInBin = @($promised | ForEach-Object { $_ -replace '\.exe$', $exeSuffix })
$absentFromBin = @()
$foundAtRoot = @()
# NOT `$name`: this script already uses that for the package name (`celira-<version>-<platform>`,
# set before the assemble step and read again when the zip is named). Reusing it here silently
# renamed the all-in-one zip to the last promised executable - measured, once.
foreach ($promisedName in $expectedInBin) {
    if ($shipped -contains $promisedName) { continue }
    if (Test-Path -LiteralPath (Join-Path $packageDir $promisedName)) { $foundAtRoot += $promisedName; continue }
    $absentFromBin += $promisedName
}
if ($absentFromBin.Count -gt 0) {
    Write-Host ('[fail] the manual promises executables that are not in this package: ' + ($absentFromBin -join ', '))
    exit 4
}
Write-Host ('[check] manual promises {0} of our executables; {1} in bin/{2}' -f `
    $expectedInBin.Count, $shipped.Count, $(if ($foundAtRoot.Count -gt 0) { (' + ' + $foundAtRoot.Count + ' at the package root (' + ($foundAtRoot -join ', ') + ')') } else { '' }))

# Package check: the same promise, second half - the manual also tells the reader to RUN scripts
# (home.js, install.ps1, serve_client.mjs, the upgrade tooling). The check above only knows about
# executables, but a missing .ps1 fails just as hard for somebody who just unzipped the package.
#
# One exception has to exist: the packager itself (scripts/make_release.ps1) is a REPO tool - it
# needs cargo and the whole tree - so the manual may name it only where it also says so out loud.
# That marker is what keeps the exception mechanical instead of a comment nobody reads: a line
# naming a repo-only script without the marker fails this check, so the manual cannot quietly tell
# a package user to run something that is not there.
$REPO_ONLY_MARKER = 'REPO-ONLY'
# The marker is spelled in Latin letters on purpose: this file is ASCII-only (see the header - a
# BOM-less .ps1 is read with the system ANSI code page), and the self-check at the top of this
# script enforces it. A Chinese marker here would arrive as mojibake.
$manualScripts = @([regex]::Matches($manualText, 'scripts[\\/][A-Za-z0-9_\\/\.\-]+\.(?:ps1|mjs|js)') |
    ForEach-Object { $_.Value -replace '/', '\' } | Sort-Object -Unique)
$missingScripts = @()
$repoOnlyScripts = @()
foreach ($relativeScript in $manualScripts) {
    if (Test-Path -LiteralPath (Join-Path $packageDir $relativeScript)) { continue }
    $leaf = Split-Path -Leaf $relativeScript
    $markedLines = @($manualText -split "`n" | Where-Object { $_.Contains($leaf) -and $_.Contains($REPO_ONLY_MARKER) })
    if ($markedLines.Count -gt 0) { $repoOnlyScripts += $relativeScript; continue }
    $missingScripts += $relativeScript
}
if ($missingScripts.Count -gt 0) {
    Write-Host ('[fail] the manual tells the reader to run scripts that are not in this package: ' + ($missingScripts -join ', '))
    Write-Host ('       either ship them, or name them on a line that carries the marker {0}' -f $REPO_ONLY_MARKER)
    exit 4
}
Write-Host ('[check] manual names {0} of our scripts; {1} in the package{2}' -f `
    $manualScripts.Count, ($manualScripts.Count - $repoOnlyScripts.Count), `
    $(if ($repoOnlyScripts.Count -gt 0) { (' + ' + $repoOnlyScripts.Count + ' repo-only, marked (' + (($repoOnlyScripts | ForEach-Object { Split-Path -Leaf $_ }) -join ', ') + ')') } else { '' }))

# Package check: the manual's first command is `node home.js up`. Same mechanical spirit as the bin/
# check above - the entry point and its modules must be in the package, not just in the repo.
$entryPoint = Join-Path $packageDir 'home.js'
$moduleDir = Join-Path $packageDir 'scripts\home'
if (-not (Test-Path $entryPoint) -or -not (Test-Path $moduleDir)) {
    Write-Host '[fail] the manual tells the user to run `node home.js up` but home.js / scripts/home are not in the package'
    exit 4
}
$moduleCount = @(Get-ChildItem $moduleDir -Filter *.js).Count
if ($moduleCount -lt 1) {
    Write-Host '[fail] scripts/home/ in the package carries no modules'
    exit 4
}
Write-Host ('[check] home entry point present: home.js + {0} module files under scripts/home/' -f $moduleCount)

# Package check: the shipped scripts/home/ must be the SAME FILE SET as the repo's (files only;
# tests/ is a subdirectory and is not shipped). Fail-closed both ways: a file the repo has but the
# package lacks is a MODULE_NOT_FOUND (or a silent data fallback) at the user's first command, and
# a file the package has but the repo does not is an artifact nobody can reproduce.
$repoModules = @(Get-ChildItem (Join-Path $PSScriptRoot 'home') -File | ForEach-Object { $_.Name } | Sort-Object)
$pkgModules = @(Get-ChildItem $moduleDir -File | ForEach-Object { $_.Name } | Sort-Object)
$moduleDiff = @(Compare-Object $repoModules $pkgModules)
if ($moduleDiff.Count -gt 0) {
    Write-Host ('[fail] scripts/home/ in the package differs from the repo: ' + (($moduleDiff | ForEach-Object { '{0}{1}' -f $_.SideIndicator, $_.InputObject }) -join ', '))
    exit 4
}
Write-Host ('[check] scripts/home/ matches the repo file-for-file: {0} files (modules + data)' -f $pkgModules.Count)

# ---------------------------------------------------------------------------
# 3a) zips carry the file mode (Unix hosts only)
# ---------------------------------------------------------------------------
# Compress-Archive stores file DATA only. On Unix the executable bit lives in each entry's
# external attributes, so a zip built without them extracts as 0644 and `node home.js up` cannot
# spawn bin/celira-im at all (found while producing the first Linux package: the archive held
# every file, none of them executable after extraction). After an archive is written, copy the
# on-disk mode of every file/directory it contains into the entry. Windows hosts have no such
# bits to carry and no API to read them, so there the zip stays exactly the Compress-Archive
# output.
function Add-UnixFileModes {
    param([string]$ZipPath, [string]$SourceDir)
    if ($hostIsWindows) { return }
    $archive = [System.IO.Compression.ZipFile]::Open($ZipPath, [System.IO.Compression.ZipArchiveMode]::Update)
    try {
        foreach ($entry in $archive.Entries) {
            $relative = $entry.FullName.TrimEnd('/')
            if (-not $relative) { continue }
            $source = Join-Path $SourceDir $relative
            if (-not (Test-Path -LiteralPath $source)) { continue }
            $mode = [int][System.IO.File]::GetUnixFileMode($source)
            $type = if (Test-Path -LiteralPath $source -PathType Container) { 0x4000 } else { 0x8000 }
            $entry.ExternalAttributes = ($type -bor $mode) -shl 16
        }
    } finally {
        $archive.Dispose()
    }
}

# ---------------------------------------------------------------------------
# 3b) split: core / home / client as independently shippable packages
# ---------------------------------------------------------------------------
# One version, four downloads. Which binary belongs to which part is DERIVED from the single
# $binaries list above (never restated here), and a binary in bin/ that the list does not know
# fails closed as "claimed by nobody" - a new executable cannot silently ship in no package.

$coreBin = @('celira-im' + $exeSuffix)
$homeBins = @($binaries | ForEach-Object { Split-Path -Leaf $_ } | Where-Object { $_ -ne ('celira-im' + $exeSuffix) })

function Test-SharedClaim {
    param([string]$Relative)
    if ($Relative -eq 'VERSION') { return $true }
    if ($Relative -eq 'scripts/serve_client.mjs') { return $true }
    if ($Relative -like 'client-dist/*') { return $true }
    return $false
}

function Get-PartClaims {
    param([string]$Relative)
    $r = $Relative -replace '\\', '/'
    if ($r -eq 'VERSION') { return @('core', 'home', 'client') }
    if ($r -like 'bin/*') {
        $leaf = Split-Path -Leaf $r
        if ($coreBin -contains $leaf) { return @('core') }
        if ($homeBins -contains $leaf) { return @('home') }
        return @()
    }
    # The world serves its own page (same-origin requirement), and the standalone client needs that
    # same page plus its server: these two are the DECLARED shared pair, claimed by two parts.
    if ($r -like 'client-dist/*' -or $r -eq 'scripts/serve_client.mjs') { return @('core', 'client') }
    if ($r -eq 'home.js' -or $r -like 'scripts/home/*') { return @('home') }
    if ($r -eq 'scripts/HOME_MANUAL.md' -or $r -eq 'docs/HOME_MANUAL.md') { return @('home') }
    # The core's deployment doc belongs to the core package (and to the all-in-one it is copied from).
    if ($r -eq 'docs/CORE_README.md') { return @('core') }
    # The desktop shell is the home's control surface (it is what starts and supervises this machine's
    # world and residents), so it ships with the home part -- the same reasoning as install.ps1 below.
    if ($r -match '^celira-app(\.exe)?$') { return @('home') }
    # The desktop walkthrough: it is the shell's manual (it tells an ordinary user what to click),
    # so it travels with the shell.
    if ($r -eq 'APP_MANUAL.md' -or $r -eq 'docs/APP_MANUAL.md') { return @('home') }
    # install.ps1 is the all-in-one installer; it ships with the home part because installing a home
    # is what it does. Its release-form detection is written for the all-in-one layout, so a
    # home-only package still needs its own install path - registered in the split report, not
    # papered over here.
    if ($r -eq 'scripts/install.ps1') { return @('home') }
    # The updater and the channel reader it calls: they act on a home's package, so they travel with
    # the home part (the same reasoning as install.ps1). update.ps1 looks for the reader at
    # <package>\scripts\probe-release-channel.mjs, which is exactly where both of them land.
    if ($r -eq 'scripts/update.ps1' -or $r -eq 'scripts/probe-release-channel.mjs') { return @('home') }
    return @()
}

$allRelative = @(Get-ChildItem -Recurse -File $packageDir | ForEach-Object {
    $_.FullName.Substring($packageDir.Length + 1) -replace '\\', '/'
})
$claims = @{}
$unclaimed = @()
foreach ($f in $allRelative) {
    $parts = @(Get-PartClaims $f)
    if ($parts.Count -eq 0) { $unclaimed += $f; continue }
    $claims[$f] = $parts
}
if ($unclaimed.Count -gt 0) {
    Write-Host ('[fail] these files belong to no split package (they would vanish from every download): ' + ($unclaimed -join ', '))
    exit 4
}
$overlap = @($claims.Keys | Where-Object { $claims[$_].Count -gt 1 -and -not (Test-SharedClaim $_) })
if ($overlap.Count -gt 0) {
    Write-Host ('[fail] undeclared overlap between split packages: ' + ($overlap -join ', '))
    exit 4
}
Write-Host ('[check] split partition: {0} files of the all-in-one package, all claimed, no undeclared overlap' -f $allRelative.Count)

$partNames = @('core', 'home', 'client')
$splitState = @()
foreach ($part in $partNames) {
    $partName = 'celira-{0}-{1}-{2}' -f $part, $version, $Platform
    $partDir = Join-Path $releaseRoot $partName
    if (Test-Path $partDir) { Remove-Item -Recurse -Force $partDir }
    New-Item -ItemType Directory -Force -Path $partDir | Out-Null
    foreach ($f in @($claims.Keys | Where-Object { $claims[$_] -contains $part })) {
        if ($f -eq 'VERSION') { continue }
        $dst = Join-Path $partDir ($f -replace '/', '\')
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
        Copy-Item (Join-Path $packageDir ($f -replace '/', '\')) $dst
    }
    Set-Content -Path (Join-Path $partDir 'VERSION') -Encoding ascii -Value (@(
        ("version = {0}" -f $version),
        ("git rev = {0}" -f $rev),
        ("built   = {0}" -f $builtAt),
        ("platform = {0}" -f $platformLabel),
        ("package = {0}" -f $part)
    ) + $channelRows + @($versionNote))
    # Same credential-leak check as the all-in-one package, per part.
    $leakedPart = @(Get-ChildItem -Recurse -File $partDir | Where-Object {
        $_.Name -match '^(home\.conf|identity\.json|facts\.ndjson|trace\.ndjson)$' -or $_.Name -like '.env*'
    })
    if ($leakedPart.Count -gt 0) {
        Write-Host ('[fail] split package {0} would contain credential-like files: {1}' -f $part, (($leakedPart | ForEach-Object { $_.Name }) -join '; '))
        exit 3
    }
    # Scoped bin/ check: the part must carry EXACTLY the executables derived from $binaries for it.
    $wantBins = @(if ($part -eq 'core') { $coreBin } elseif ($part -eq 'home') { $homeBins } else { @() })
    $actualBins = @(Get-ChildItem (Join-Path $partDir 'bin') -Filter ('*' + $exeSuffix) -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Name } | Sort-Object)
    $diff = @(Compare-Object @($wantBins | Sort-Object) $actualBins)
    if ($diff.Count -gt 0) {
        Write-Host ('[fail] split package {0} carries the wrong executables: {1}' -f $part, (($diff | ForEach-Object { $_.InputObject }) -join ', '))
        exit 4
    }
    $partCount = @(Get-ChildItem -Recurse -File $partDir).Count
    if ($partCount -lt 1) {
        Write-Host ('[fail] split package {0} is empty' -f $part)
        exit 4
    }
    $partZip = Join-Path $releaseRoot ($partName + '.zip')
    if (Test-Path $partZip) { Remove-Item -Force $partZip }
    Compress-Archive -Path (Join-Path $partDir '*') -DestinationPath $partZip
    Add-UnixFileModes -ZipPath $partZip -SourceDir $partDir
    $partManifest = @(Get-ChildItem -Recurse -File $partDir | Sort-Object FullName | ForEach-Object {
        $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
        $rel = $_.FullName.Substring($partDir.Length + 1)
        ('{0}  {1}' -f $hash.ToLower(), ($rel -replace '\\', '/'))
    })
    Set-Content -Path (Join-Path $releaseRoot ('MANIFEST-{0}-{1}.txt' -f $part, $version)) -Encoding ascii -Value $partManifest
    $splitState += [pscustomobject]@{
        part  = $part
        count = $partCount
        bins  = $actualBins.Count
        zip   = $partZip
        bytes = (Get-Item $partZip).Length
    }
}

# ---------------------------------------------------------------------------
# 4) zip
# ---------------------------------------------------------------------------

$zip = Join-Path $releaseRoot ($name + '.zip')
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $packageDir '*') -DestinationPath $zip
Add-UnixFileModes -ZipPath $zip -SourceDir $packageDir
$zipBytes = (Get-Item $zip).Length

# The digest is computed ONCE, here, and then used by every channel. A release that is offered
# by two channels but has two different digests is not a mirror - it is two products with one
# version number, and an updater that verifies bytes must refuse it.
$zipHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
# A sidecar file next to the zip: the name makes it obvious which asset it describes, and the
# same file is uploaded to both channels so a human can check either one.
$sumsFile = Join-Path $releaseRoot ($name + '.zip.sha256')
Set-Content -Path $sumsFile -Encoding ascii -Value ('{0}  {1}' -f $zipHash, (Split-Path -Leaf $zip))

Write-Host ''
Write-Host '[state]'
Write-Host ("       package   = {0}" -f $packageDir)
Write-Host ("       zip       = {0} ({1:N1} MiB)" -f $zip, ($zipBytes / 1MB))
Write-Host ("       sha256    = {0}" -f $zipHash)
Write-Host ("       files     = {0}" -f (@(Get-ChildItem -Recurse -File $packageDir).Count))
Write-Host ("       manifest  = {0}" -f (Join-Path $releaseRoot ('MANIFEST-all-in-one-{0}.txt' -f $version)))
Write-Host ''
Write-Host '[split]  one version, four downloads (all-in-one is the default download; the other three are its parts)'
foreach ($row in $splitState) {
    Write-Host ("       {0,-7} = {1,4} files, {2} exe, {3,7:N1} MiB  {4}" -f $row.part, $row.count, $row.bins, ($row.bytes / 1MB), $row.zip)
}
Write-Host '       shared  = client-dist/**, scripts/serve_client.mjs (core + client), VERSION (all four)'

# Manifest: sha256 per file (the idempotency check compares two builds through it; VERSION carries
# the build time, which is the one line that legitimately differs between runs)
$manifest = @(Get-ChildItem -Recurse -File $packageDir | Sort-Object FullName | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
    $relative = $_.FullName.Substring($packageDir.Length + 1)
    ('{0}  {1}' -f $hash.ToLower(), ($relative -replace '\\', '/'))
})
Set-Content -Path (Join-Path $releaseRoot 'MANIFEST.txt') -Encoding ascii -Value $manifest
# MANIFEST.txt keeps its documented name (= this run's all-in-one), but the directory holds every
# version ever built - so each run also leaves an immutable per-version record.
Set-Content -Path (Join-Path $releaseRoot ('MANIFEST-all-in-one-{0}.txt' -f $version)) -Encoding ascii -Value $manifest

# ---------------------------------------------------------------------------
# 5) publish
# ---------------------------------------------------------------------------

if ($SkipPublish) {
    Write-Host ''
    Write-Host '[publish] skipped (-SkipPublish): the package is kept, nothing was published'
    exit 0
}

# Which channel are we publishing to? The origin decides which one is primary. `-AlsoGitee`
# mirrors the same bytes to Gitee after GitHub. (PROJECT_SHAPE.md section 7 gap 6 was "the release
# channel is not ruled on"; the ruling since 2026-10-21 is: GitHub Releases is primary, Gitee is a
# mirror of the SAME bytes - not a second build.)
Write-Host ''
Write-Host ("[publish] origin = {0} ({1})" -f $(if ($originUrl) { $originUrl } else { '(no origin)' }), $originHost)
if ($GiteeRepo) { Write-Host ("[publish] gitee  = {0}" -f $GiteeRepo) }

# Release notes are the changelog: the commit subjects in this version (Conventional Commits
# lines), so there is no separate CHANGELOG.md to keep in sync. The digest line is appended here
# so BOTH channels carry it in their release body - that is what makes the body machine-readable
# for the updater (the Gitee API reports no asset digest).
$previous = (& git -C $root describe --tags --abbrev=0 2>$null)
$range = if ($previous) { '{0}..HEAD' -f $previous } else { 'HEAD' }
$notes = @((& git -C $root log --pretty=format:%s $range 2>$null) | Where-Object { $_ -match '^\w+(\(.+\))?:' })
$notesFile = Join-Path $releaseRoot 'RELEASE_NOTES.md'
Set-Content -Path $notesFile -Encoding utf8 -Value (@(
    ("# Celira {0}" -f $version),
    '',
    ("Built from {0} on {1}." -f $rev, $builtAt),
    '',
    '## Changes in this release',
    ''
) + ($notes | ForEach-Object { '- ' + $_ }) + @(
    '',
    ('sha256: {0}' -f $zipHash)
))

function Invoke-GiteePublish {
    $script = Join-Path $PSScriptRoot 'publish-gitee.ps1'
    & $script -Tag $Tag -Zip $zip -Repo $GiteeRepo -NotesFile $notesFile `
        -Title ("Celira {0}" -f $version)
    return $LASTEXITCODE
}

if ($originHost -eq 'gitee') {
    if (-not $GiteeRepo) {
        Write-Host '[publish] NOT PUBLISHED: origin is gitee.com but no release repository could be read'
        Write-Host ("          the package is ready at {0}" -f $zip)
        exit 5
    }
    $code = Invoke-GiteePublish
    if ($code -ne 0) {
        Write-Host ('[publish] NOT PUBLISHED: publish-gitee.ps1 exited {0}' -f $code)
        Write-Host ("          the package is ready at {0}" -f $zip)
        exit 5
    }
    Write-Host ("[publish] published on gitee: {0}" -f $Tag)
    exit 0
}

$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) {
    Write-Host ''
    Write-Host '[publish] NOT PUBLISHED: the GitHub CLI (gh) was not found on PATH'
    Write-Host ("          the package is ready at {0}" -f $zip)
    Write-Host ("          publish it manually once gh is available: gh release create {0} `"{1}`" --notes-file <notes>" -f $Tag, $zip)
    exit 5
}

Write-Host ''
Write-Host ("[publish] gh release create {0} ({1} note line(s), zip + sha256)" -f $Tag, $notes.Count)
& gh release create $Tag $zip $sumsFile --title ("Celira {0}" -f $version) --notes-file $notesFile
if ($LASTEXITCODE -ne 0) {
    Write-Host '[publish] NOT PUBLISHED: gh refused (no permission, not signed in, or the remote is not GitHub)'
    Write-Host ("          the package is ready at {0}" -f $zip)
    exit 5
}
Write-Host ("[publish] published on github: {0}" -f $Tag)

if ($AlsoGitee) {
    if (-not $GiteeRepo) {
        Write-Host '[publish] GitHub is published, but -AlsoGitee needs -GiteeRepo <owner/name>'
        Write-Host '[publish] the two channels are now out of step: github has this version, gitee does not'
        exit 5
    }
    $code = Invoke-GiteePublish
    if ($code -ne 0) {
        Write-Host ('[publish] GitHub published, Gitee FAILED (exit {0}) -- the two channels are out of step' -f $code)
        exit 5
    }
    Write-Host ("[publish] mirrored on gitee: {0}" -f $Tag)
}

exit 0
