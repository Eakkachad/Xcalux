# Builds the Community Preview zip: one file a tester unzips and double-clicks.
#
#   .\scripts\package.ps1              regenerate the notices, release build, zip into dist\
#   .\scripts\package.ps1 -SkipBuild   zip the arty.exe that is already built
#
# dist\ARTY-Preview0-<version>-<git hash>-win64.zip holds arty.exe, THIRD_PARTY_NOTICES.txt,
# LICENSE-PREVIEW.txt, README-TH.txt and README-EN.txt (templates in scripts\package\).
# arty.pdb is copied next to the zip, NOT into it: keep it with the build, it turns the
# addresses in a tester's crash report into function names.
# Checks that arty.exe imports no Visual C++ runtime DLL (E14: +crt-static) and fails if it does.
# dist\ is git-ignored.
param(
    [switch]$SkipBuild,
    [string]$OutDir = ''
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $root 'dist' }
$release = 'ARTY Preview 0'

$version = [regex]::Match((Get-Content -Raw (Join-Path $root 'Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw 'no version in Cargo.toml' }
Push-Location $root
try {
    $hash = (git rev-parse --short=8 HEAD).Trim()
    $dirty = [bool](git status --porcelain --untracked-files=no)
    if ($dirty) { Write-Warning "the working tree has uncommitted changes: this zip is $hash plus changes" }

    & (Join-Path $PSScriptRoot 'notices.ps1')
    if (-not $SkipBuild) {
        # cargo prints its progress on stderr, which Windows PowerShell 5.1 would treat as an error.
        $ErrorActionPreference = 'Continue'
        cargo build --release -p arty-app 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        if ($code -ne 0) { throw 'cargo build failed' }
    }
} finally { Pop-Location }

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$exe = Join-Path $targetDir 'release\arty.exe'
if (-not (Test-Path $exe)) { throw "no ${exe}: build first" }

# Imported DLLs of a 64-bit PE file (normal and delay-load imports).
function Get-PeImports([string]$path) {
    $b = [IO.File]::ReadAllBytes($path)
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    if ([BitConverter]::ToUInt32($b, $pe) -ne 0x00004550) { throw 'not a PE file' }
    $nsec = [BitConverter]::ToUInt16($b, $pe + 6)
    $optSize = [BitConverter]::ToUInt16($b, $pe + 20)
    $opt = $pe + 24
    if ([BitConverter]::ToUInt16($b, $opt) -ne 0x20B) { throw 'not a 64-bit PE file' }
    $secs = @()
    for ($i = 0; $i -lt $nsec; $i++) {
        $s = $opt + $optSize + 40 * $i
        $secs += , @([BitConverter]::ToUInt32($b, $s + 12), [Math]::Max([BitConverter]::ToUInt32($b, $s + 8), [BitConverter]::ToUInt32($b, $s + 16)), [BitConverter]::ToUInt32($b, $s + 20))
    }
    function ToOffset([uint32]$rva) {
        foreach ($s in $secs) { if ($rva -ge $s[0] -and $rva -lt ($s[0] + $s[1])) { return [int]($rva - $s[0] + $s[2]) } }
        -1
    }
    function Str([int]$off) { $e = $off; while ($b[$e] -ne 0) { $e++ }; [Text.Encoding]::ASCII.GetString($b, $off, $e - $off) }
    $names = @()
    # data directory 1 = import, 13 = delay import
    foreach ($d in @(@(1, 20, 12), @(13, 32, 4))) {
        $rva = [BitConverter]::ToUInt32($b, $opt + 112 + 8 * $d[0])
        if ($rva -eq 0) { continue }
        $off = ToOffset $rva
        while ($off -ge 0) {
            $nameRva = [BitConverter]::ToUInt32($b, $off + $d[2])
            if ($nameRva -eq 0) { break }
            $n = ToOffset $nameRva
            if ($n -lt 0) { break }
            $names += Str $n
            $off += $d[1]
        }
    }
    $names | Sort-Object -Unique
}

$imports = @(Get-PeImports $exe)
"imports of arty.exe: $($imports -join ', ')"
$crt = @($imports | Where-Object { $_ -match '^(vcruntime|msvcp|vcomp|concrt|msvcr|vccorlib|ucrtbase|api-ms-win-crt)' })
if ($crt.Count) { throw "arty.exe depends on the C runtime DLL(s): $($crt -join ', '); a clean Windows 10 may not have them (build with +crt-static, .cargo\config.toml)" }
'no C runtime DLL dependency (static CRT)'

$name = "ARTY-Preview0-$version-$hash-win64"
$stage = Join-Path $OutDir $name
$zip = Join-Path $OutDir "$name.zip"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $zip) { Remove-Item -Force $zip }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe (Join-Path $stage 'arty.exe')
Copy-Item (Join-Path $root 'THIRD_PARTY_NOTICES.txt') $stage

# Text for Notepad: UTF-8 with a BOM (Thai) and Windows line endings; placeholders filled in.
$utf8bom = New-Object System.Text.UTF8Encoding($true)
function Write-Doc([string]$src, [string]$dst) {
    $t = [IO.File]::ReadAllText($src).Replace("`r`n", "`n")
    $t = $t.Replace('{RELEASE}', $release).Replace('{VERSION}', $version).Replace('{HASH}', $hash).Replace("`n", "`r`n")
    [IO.File]::WriteAllText($dst, $t, $utf8bom)
}
foreach ($f in 'LICENSE-PREVIEW.txt', 'README-TH.txt', 'README-EN.txt') {
    Write-Doc (Join-Path $PSScriptRoot "package\$f") (Join-Path $stage $f)
}

Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
$pdb = Join-Path $targetDir 'release\arty.pdb'
if (Test-Path $pdb) { Copy-Item $pdb (Join-Path $OutDir "$name.pdb") -Force }

''
'zip contents:'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$za = [IO.Compression.ZipFile]::OpenRead($zip)
try { $za.Entries | ForEach-Object { '  {0,-26} {1,12:N0} bytes' -f $_.FullName, $_.Length } } finally { $za.Dispose() }
''
'{0}: {1:N2} MiB ({2:N0} bytes)' -f $zip, ((Get-Item $zip).Length / 1MB), (Get-Item $zip).Length
'arty.exe: {0:N2} MiB ({1:N0} bytes)' -f ((Get-Item $exe).Length / 1MB), (Get-Item $exe).Length
if ($dirty) { Write-Warning 'built from a working tree with uncommitted changes' }
