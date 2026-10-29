# Fetches the bundled open-source fonts into assets/fonts (not in git; about 76 MB).
# Run once before a dev run and before building the installer. Files already present with the
# right hash are skipped. All three files are SIL OFL 1.1 (see THIRD_PARTY_NOTICES.txt).
# Uses curl.exe because Windows PowerShell 5.1 Invoke-WebRequest fails on GitHub release assets here,
# and the Windows tar.exe (bsdtar) to read the .7z.
$dir = "$PSScriptRoot\..\assets\fonts"
New-Item -ItemType Directory -Force $dir | Out-Null
$sarasa = @{
    # UI, DISPLAY, MONO: Sarasa Mono SC (CJK exactly two cells wide), be5invis/Sarasa-Gothic v1.0.43
    Url = 'https://github.com/be5invis/Sarasa-Gothic/releases/download/v1.0.43/SarasaMonoSC-TTF-1.0.43.7z'
    ArchiveSha256 = '54747ee26f0bc47dc85583df5f8877a4607467afd70630b80ab4505cccaa8e1d'
    Files = @(
        @{ Name = 'SarasaMonoSC-Regular.ttf'; Sha256 = 'f0db6e00d3f9ffc3643430483410fd463758d8da93204f1ed1d30278afc2db28' },
        @{ Name = 'SarasaMonoSC-Bold.ttf'; Sha256 = 'd1adcd43e438d94143f8f4dabaa4892aa847bfdee59f344adefd2c8e9514ba3a' }
    )
}
# PROSE: LXGW WenKai Mono Regular, lxgw/LxgwWenKai v1.530 (a plain .ttf release asset)
$lxgw = @{ Name = 'LXGWWenKaiMono-Regular.ttf'
    Url = 'https://github.com/lxgw/LxgwWenKai/releases/download/v1.530/LXGWWenKaiMono-Regular.ttf'
    Sha256 = '895ce1d2fa00d5ca3bec0329d38e3190454f4b5fa7b4733f491c419faf9753a1' }

function Test-Font($path, $sha) { (Test-Path $path) -and (Get-FileHash $path -Algorithm SHA256).Hash -eq $sha }
function Get-Asset($url, $out, $sha) {
    & curl.exe -sSL --fail -o $out $url
    if ($LASTEXITCODE) { throw "download failed: $url" }
    $h = (Get-FileHash $out -Algorithm SHA256).Hash
    if ($h -ne $sha) { throw "$url SHA-256 mismatch: $h" }
}

# LXGW WenKai Mono
$p = Join-Path $dir $lxgw.Name
if (-not (Test-Font $p $lxgw.Sha256)) {
    $tmp = "$p.part"
    try { Get-Asset $lxgw.Url $tmp $lxgw.Sha256; Move-Item -Force $tmp $p } finally { if (Test-Path $tmp) { [IO.File]::Delete($tmp) } }
}

# Sarasa Mono SC: download the family archive only when a file is missing
if ($sarasa.Files | Where-Object { -not (Test-Font (Join-Path $dir $_.Name) $_.Sha256) }) {
    $work = Join-Path ([IO.Path]::GetTempPath()) ("claudetile-font-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory $work | Out-Null
    try {
        $arc = Join-Path $work 'sarasa.7z'
        Get-Asset $sarasa.Url $arc $sarasa.ArchiveSha256
        $out = Join-Path $work 'out'
        New-Item -ItemType Directory $out | Out-Null
        & "$env:SystemRoot\System32\tar.exe" -xf $arc -C $out ($sarasa.Files | ForEach-Object { $_.Name })
        if ($LASTEXITCODE) { throw 'tar.exe could not extract the .7z' }
        foreach ($f in $sarasa.Files) {
            $src = Join-Path $out $f.Name
            $h = (Get-FileHash $src -Algorithm SHA256).Hash
            if ($h -ne $f.Sha256) { throw "$($f.Name) SHA-256 mismatch: $h" }
            Move-Item -Force $src (Join-Path $dir $f.Name)
        }
    } finally { [IO.Directory]::Delete($work, $true) }
}
