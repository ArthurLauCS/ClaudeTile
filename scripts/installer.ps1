# 编译 release 再打安装包，产物在 target\installer\ClaudeTile-Setup-<版本>.exe。需要 Inno Setup 6。
$root = "$PSScriptRoot\.."
$out = "$root\target\installer"
# 只留本次版本的产物，旧版本的安装包不累计
if (Test-Path $out) { [IO.Directory]::Delete($out, $true) }
# 随程序带的字体（更纱等距黑体、霞鹜文楷等宽），已有且哈希对得上就跳过
& powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\fonts.ps1"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
cargo build --release --manifest-path "$root\Cargo.toml"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
# Inno Setup 可能装在全机或当前用户目录下
$iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
& $iscc /Q "/DVersion=$version" "$root\installer\claudetile.iss"
exit $LASTEXITCODE
