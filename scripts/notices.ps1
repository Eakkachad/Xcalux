# Regenerates THIRD_PARTY_NOTICES.txt for the release binary (arty.exe).
#
#   .\scripts\notices.ps1            write THIRD_PARTY_NOTICES.txt at the repo root
#   .\scripts\notices.ps1 -Check     only compare: exit 1 when the file in the repo is out of date
#
# The crate list is `cargo tree -p arty-app -e normal` for x86_64-pc-windows-msvc (what is linked
# into the exe; proc-macro crates are listed too, which is harmless); licence, authors and the
# manifest folder come from `cargo metadata`. For each crate the file gets its licence expression,
# authors and every copyright line found in its LICENSE / COPYING / NOTICE files. The full text of
# each licence that applies is written once (an "A OR B" crate is taken under the first of its
# alternatives in a fixed order: MIT, ISC, BSD, Zlib, ..., Apache-2.0), plus the bundled fonts' own
# licences. Crates under GPL / LGPL / AGPL, or with no licence at all, are reported at the end.
# Nothing here changes a dependency. The output has no date, so an unchanged tree gives an
# unchanged file. Needs the crates in the cargo registry (run `cargo fetch` once online).
# The text is compiled into the exe (src/about.rs, Help > About > Licences).
param([switch]$Check)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root 'THIRD_PARTY_NOTICES.txt'
$target = 'x86_64-pc-windows-msvc'
Push-Location $root
$prevEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)   # authors with accents
try {
    $tree = @(cargo tree -p arty-app -e normal --target $target --prefix none --offline -f '{p}' 2>$null)
    if ($LASTEXITCODE -ne 0) { throw 'cargo tree failed' }
    $metaJson = (cargo metadata --format-version 1 --offline --filter-platform $target 2>$null) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
} finally { Pop-Location; [Console]::OutputEncoding = $prevEncoding }
$meta = $metaJson | ConvertFrom-Json

# name -> package for the crates in the tree (registry crates only; ARTY's own are described below).
$byKey = @{}
foreach ($p in $meta.packages) { $byKey["$($p.name) $($p.version)"] = $p }
$crates = @{}
foreach ($line in $tree) {
    if ($line -match '^(\S+) v(\S+)') {
        $pkg = $byKey["$($Matches[1]) $($Matches[2])"]
        if (-not $pkg) { throw "crate $($Matches[1]) $($Matches[2]) is not in cargo metadata" }
        if ($pkg.source) { $crates["$($pkg.name) $($pkg.version)"] = $pkg }
    }
}
$packages = @($crates.Values | Sort-Object { $_.name }, { $_.version })

# ---- licence expressions -------------------------------------------------------------------
# Order in which an OR is resolved; anything not listed comes last.
$prefer = @('MIT', 'ISC', 'BSD-2-Clause', 'BSD-3-Clause', 'Zlib', '0BSD', 'Unlicense', 'Apache-2.0', 'BSL-1.0', 'Unicode-3.0', 'OFL-1.1', 'Ubuntu-font-1.0')
function Rank($id) { $i = $prefer.IndexOf($id); if ($i -lt 0) { 99 } else { $i } }
function Test-Copyleft($id) { $id -match '(^|-)(GPL|LGPL|AGPL)' -or $id -match '^(GPL|LGPL|AGPL)' }

# Splits at `word` (OR / AND) outside parentheses.
function Split-Top([string]$s, [string]$word) {
    $parts = @(); $depth = 0; $start = 0
    for ($i = 0; $i -lt $s.Length; $i++) {
        $c = $s[$i]
        if ($c -eq '(') { $depth++ } elseif ($c -eq ')') { $depth-- }
        elseif ($depth -eq 0 -and $c -eq ' ' -and $s.Substring($i).StartsWith(" $word ")) {
            $parts += $s.Substring($start, $i - $start); $start = $i + $word.Length + 2; $i = $start - 1
        }
    }
    $parts += $s.Substring($start)
    , $parts
}

# SPDX expression -> alternatives, each a string of the licence ids that all apply ("MIT OFL-1.1").
function Parse-License([string]$expr) {
    $e = ($expr -replace '/', ' OR ').Trim()
    $ors = Split-Top $e 'OR'
    if ($ors.Count -gt 1) { return @($ors | ForEach-Object { Parse-License $_ }) }
    $ands = Split-Top $e 'AND'
    if ($ands.Count -gt 1) {
        $acc = @('')
        foreach ($part in $ands) {
            $alts = @(Parse-License $part)
            $acc = @($acc | ForEach-Object { $a = $_; $alts | ForEach-Object { ("$a $_").Trim() } })
        }
        return $acc
    }
    if ($e.StartsWith('(') -and $e.EndsWith(')')) { return @(Parse-License $e.Substring(1, $e.Length - 2)) }
    return @($e)
}

# The alternative a crate is taken under: no copyleft id if one exists, then the best-ranked.
function Choose-License([string]$expr) {
    $best = $null; $bestScore = 1000
    foreach ($alt in @(Parse-License $expr)) {
        $ids = @($alt -split ' ' | Where-Object { $_ })
        $score = ($ids | ForEach-Object { Rank $_ } | Measure-Object -Maximum).Maximum
        if ($ids | Where-Object { Test-Copyleft $_ }) { $score += 500 }
        if ($score -lt $bestScore) { $best = $ids; $bestScore = $score }
    }
    $best
}

# ---- licence texts -------------------------------------------------------------------------
$text = @{}
$text['MIT'] = @'
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
'@
$text['ISC'] = @'
Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
'@
$bsdHead = @'
Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

'@
$bsdThird = @'
3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

'@
$bsdTail = @'
THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
'@
$text['BSD-2-Clause'] = $bsdHead + $bsdTail
$text['BSD-3-Clause'] = $bsdHead + $bsdThird + $bsdTail
$text['Zlib'] = @'
This software is provided 'as-is', without any express or implied
warranty. In no event will the authors be held liable for any damages
arising from the use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not
   claim that you wrote the original software. If you use this software
   in a product, an acknowledgment in the product documentation would be
   appreciated but is not required.
2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.
3. This notice may not be removed or altered from any source distribution.
'@
$text['0BSD'] = @'
Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
'@
$text['BSL-1.0'] = @'
Boost Software License - Version 1.0 - August 17th, 2003

Permission is hereby granted, free of charge, to any person or organization
obtaining a copy of the software and accompanying documentation covered by
this license (the "Software") to use, reproduce, display, distribute,
execute, and transmit the Software, and to prepare derivative works of the
Software, and to permit third-parties to whom the Software is furnished to
do so, all subject to the following:

The copyright notices in the Software and this entire statement, including
the above license grant, this restriction and the following disclaimer,
must be included in all copies of the Software, in whole or in part, and
all derivative works of the Software, unless such copies or derivative
works are solely in the form of machine-executable object code generated by
a source language processor.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT
SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE LIABLE
FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
'@

function Crate-Dir($name) { $p = $packages | Where-Object { $_.name -eq $name } | Select-Object -First 1; if ($p) { Split-Path -Parent $p.manifest_path } }
function Read-Text($path) { [IO.File]::ReadAllText($path).Replace("`r`n", "`n").TrimEnd() }

# Licences whose text is taken from a file the exe's own sources or crates carry.
$apacheFile = $packages | ForEach-Object { Join-Path (Split-Path -Parent $_.manifest_path) 'LICENSE-APACHE' } |
    Where-Object { (Test-Path $_) -and ((Read-Text $_) -match 'END OF TERMS AND CONDITIONS') } | Select-Object -First 1
if ($apacheFile) {
    $a = Read-Text $apacheFile
    $text['Apache-2.0'] = $a.Substring(0, $a.IndexOf('END OF TERMS AND CONDITIONS') + 'END OF TERMS AND CONDITIONS'.Length).TrimStart()
}
$unicodeDir = Crate-Dir 'unicode-ident'
if ($unicodeDir -and (Test-Path "$unicodeDir\LICENSE-UNICODE")) { $text['Unicode-3.0'] = Read-Text "$unicodeDir\LICENSE-UNICODE" }
$fontsDir = Join-Path (Crate-Dir 'epaint_default_fonts') 'fonts'
$thaiOfl = Join-Path $root 'crates\arty-app\assets\fonts\OFL.txt'
# OFL-1.1: the licence part of the Noto OFL.txt (its first lines are the font's own copyright).
$ofl = Read-Text $thaiOfl
$text['OFL-1.1'] = $ofl.Substring($ofl.IndexOf('SIL OPEN FONT LICENSE Version 1.1')).TrimEnd()
$text['Ubuntu-font-1.0'] = Read-Text (Join-Path $fontsDir 'UFL.txt')

# ---- per-crate lines -----------------------------------------------------------------------
function Wrap([string]$s, [int]$width, [string]$indent) {
    $lines = @(); $cur = ''
    $lead = $s.Substring(0, $s.Length - $s.TrimStart().Length)
    foreach ($w in ($s.Trim() -split '\s+')) {
        if ($cur.Length -gt 0 -and ($cur.Length + 1 + $w.Length) -gt $width) { $lines += $cur; $cur = $indent + $w }
        elseif ($cur.Length -eq 0) { $cur = $lead + $w } else { $cur += ' ' + $w }
    }
    if ($cur) { $lines += $cur }
    $lines
}

function Copyright-Lines($dir) {
    $found = New-Object System.Collections.Generic.List[string]
    Get-ChildItem -File $dir -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|UNLICENSE|NOTICE)' } | ForEach-Object {
        foreach ($l in ([IO.File]::ReadAllLines($_.FullName))) {
            $t = $l.Trim()
            if ($t -cmatch '^(Copyright|Portions [Cc]opyright|\([cC]\) ?[0-9]|[^\x00-\x7F] ?[0-9])' -and $t -cnotmatch '\[yyyy\]|<year>|<copyright|\{yyyy|YEAR|^Copyright and|^Copyright notice|holder>|owner>' -and $t.Length -lt 200) {
                if (-not $found.Contains($t)) { $found.Add($t) }
            }
        }
    }
    @($found | Select-Object -First 4)
}

$sb = New-Object System.Text.StringBuilder
function Add([string]$s = '') { [void]$sb.Append($s).Append("`n") }
# A licence text, long paragraphs wrapped (the About dialog draws one row per line).
function Add-Text([string]$s) {
    foreach ($l in ($s -split "`n")) {
        if ($l.Length -le 110) { Add $l } else { foreach ($w in (Wrap $l 100 '')) { Add $w } }
    }
}
function Rule([string]$title) { Add ('=' * 78); Add $title; Add ('=' * 78); Add }

Add 'ARTY - third-party notices'
Add
Add 'ARTY is built with the open-source software listed below. Each part stays under its own'
Add 'licence; the licence texts follow the list. Generated by scripts\notices.ps1 from the'
Add 'dependency tree of arty.exe; do not edit by hand.'
Add

Rule 'Included with ARTY itself'
Add 'ARTY Preview 0'
Add '  ARTY itself is not open source: see LICENSE-PREVIEW.txt (free for personal testing, do not'
Add '  redistribute, all rights reserved by the owner).'
Add
Add 'hokusai 0.2.0 (brush engine)'
Add '  A Rust port of the libmypaint brush engine (libmypaint is under the ISC licence), taken'
Add '  as source into this project. Licensed under MIT OR Apache-2.0 for the Rust code;'
Add '  the libmypaint-derived parts below are under ISC.'
Add '  Copyright (c) 2026 Re:Earth and contributors (MIT)'
Add '  libmypaint: Copyright (c) 2007-2018 Martin Renold, Jon Nordby and the MyPaint /'
Add '  libmypaint contributors (ISC) - https://github.com/mypaint/libmypaint'
Add
Add 'arty-smart (assist math)'
Add '  Parts are ported from katgpt-rs (https://github.com/katopz/katgpt-rs), MIT licence:'
Add '  Copyright (c) 2026 Todsaporn Banjerdkit'
Add
Add 'Noto Sans Thai UI (the Thai interface font, compiled into the exe)'
Add '  Copyright 2012 Google Inc. All Rights Reserved. SIL Open Font License 1.1.'
Add
Add 'Phosphor Icons (the icon font of the egui-phosphor crate, compiled into the exe)'
Add '  Copyright (c) 2020 Phosphor Icons. MIT licence.'
Add
Add 'Fonts of the egui default set (epaint_default_fonts, compiled into the exe)'
Add '  Hack: Copyright 2018 Source Foundry Authors (MIT); based on DejaVu (public domain) and'
Add '    Bitstream Vera Sans Mono, Copyright 2003 Bitstream Inc. (Bitstream Vera licence).'
Add '  Noto Emoji: Copyright 2012 Google Inc. (SIL OFL 1.1).'
Add '  Ubuntu Light: Copyright 2010, 2011 Canonical Ltd and Dalton Maag Ltd (Ubuntu Font Licence 1.0).'
Add '  emoji-icon-font: MIT licence (see "Bundled font licences" below).'
Add

Rule 'Crates'
$flags = New-Object System.Collections.Generic.List[string]
$used = New-Object System.Collections.Generic.SortedSet[string]
foreach ($p in $packages) {
    $dir = Split-Path -Parent $p.manifest_path
    $expr = $p.license
    if (-not $expr) { $flags.Add("FLAG unknown licence: $($p.name) $($p.version)"); $expr = '(none declared)' }
    $title = "$($p.name) $($p.version)"
    Add $title
    $chosen = @()
    if ($expr -ne '(none declared)') {
        $chosen = @(Choose-License $expr)
        foreach ($id in $chosen) { [void]$used.Add($id) }
        if ($expr -match 'GPL') {
            if ($chosen | Where-Object { Test-Copyleft $_ }) { $flags.Add("FLAG copyleft only: $title ($expr)") }
            else { $flags.Add("note: $title is $expr, used under $($chosen -join ' + ')") }
        }
        if ($expr -match 'MPL|EPL|CDDL') { $flags.Add("note: $title is $expr (file-level copyleft)") }
    }
    $licLine = "  Licence: $expr"
    if ($expr -match ' OR ' -and $chosen.Count) { $licLine += "  (used under $($chosen -join ' + '))" }
    foreach ($l in (Wrap $licLine 100 '    ')) { Add $l }
    $authors = @($p.authors | Where-Object { $_ })
    if ($authors.Count) { foreach ($l in (Wrap ('  Authors: ' + ($authors -join ', ')) 100 '    ')) { Add $l } }
    $copy = Copyright-Lines $dir
    foreach ($c in $copy) { foreach ($l in (Wrap ('  ' + $c) 100 '    ')) { Add $l } }
    if (-not $copy.Count -and -not $authors.Count) { $flags.Add("note: no copyright line or author in the package of $title") }
    if ($p.repository) { Add "  Source: $($p.repository)" }
    Add
}

[void]$used.Add('ISC')   # hokusai / libmypaint
[void]$used.Add('MIT')
[void]$used.Add('OFL-1.1')

Rule 'Licence texts'
Add 'The copyright lines of each crate are in the list above (or, where its package carries none,'
Add 'its authors); the text below applies to every part that names the licence.'
Add
foreach ($id in $used) {
    Add ('-' * 78)
    Add $id
    Add ('-' * 78)
    Add
    if ($text.ContainsKey($id)) { Add-Text $text[$id] }
    else { Add "(text of $id not found by scripts\notices.ps1)"; $flags.Add("FLAG no licence text for $id") }
    Add
}

Rule 'Bundled font licences'
foreach ($f in 'Hack-Regular.txt', 'emoji-icon-font-mit-license.txt') {
    Add ('-' * 78); Add "epaint_default_fonts: $f"; Add ('-' * 78); Add
    Add-Text (Read-Text (Join-Path $fontsDir $f))
    Add
}
Add ('-' * 78); Add 'Noto Sans Thai UI: OFL.txt'; Add ('-' * 78); Add
Add-Text $ofl

$content = $sb.ToString().TrimEnd() + "`n"
$utf8 = New-Object System.Text.UTF8Encoding($false)
if ($Check) {
    $old = if (Test-Path $out) { [IO.File]::ReadAllText($out).Replace("`r`n", "`n") } else { '' }
    if ($old -ne $content) { Write-Warning 'THIRD_PARTY_NOTICES.txt is out of date: run scripts\notices.ps1'; $global:LASTEXITCODE = 1 }
    else { 'THIRD_PARTY_NOTICES.txt is up to date' }
} else {
    [IO.File]::WriteAllText($out, $content, $utf8)
    "wrote $out ($([math]::Round($content.Length / 1KB)) KB, $($packages.Count) crates, licences: $($used -join ', '))"
}
if ($flags.Count) { ''; 'Licence report:'; $flags | ForEach-Object { "  $_" } } else { 'Licence report: no GPL / LGPL / AGPL / unknown licences' }
