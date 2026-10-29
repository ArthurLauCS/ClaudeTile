; ClaudeTile 安装包（Inno Setup 6）。用 scripts\installer.ps1 构建：它先编译，再传入版本号。
; 按当前用户安装到 %LOCALAPPDATA%\Programs\ClaudeTile，不要管理员权限。

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId=ClaudeTile
AppName=ClaudeTile
AppVersion={#Version}
AppVerName=ClaudeTile {#Version}
AppPublisher=ArthurLau
AppPublisherURL=https://github.com/ArthurLauCS/ClaudeTile
DefaultDirName={autopf}\ClaudeTile
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\LICENSE
OutputDir=..\target\installer
OutputBaseFilename=ClaudeTile-Setup-{#Version}
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\claudetile.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; 每次都让用户选语言；第一次安装时选中的语言也是程序的界面语言
UsePreviousLanguage=no
; 升级时关掉正在运行的实例（它收到 WM_CLOSE 会先存好用量历史再退出）
CloseApplications=force
RestartApplications=no

[Languages]
Name: "zh"; MessagesFile: "ChineseSimplified.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
zh.AutoStart=开机自动启动
en.AutoStart=Start with Windows
zh.DeleteData=同时删除设置和 Token 用量历史吗？%n%n用量历史里有 Claude Code 已经清理掉的旧记录，删掉后热力图就找不回来了。重装时保留它们可以接着用。
en.DeleteData=Also delete your settings and token usage history?%n%nThe history keeps days that Claude Code has already cleaned up; once deleted, the heatmap cannot get them back. Keep them if you plan to reinstall.

[Tasks]
Name: "autostart"; Description: "{cm:AutoStart}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "..\target\release\claudetile.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
; 字体由 scripts\fonts.ps1 下载到 assets\fonts；缺了字体程序会退回系统字体，所以先跑那个脚本再打包
Source: "..\assets\fonts\*.ttf"; DestDir: "{app}\fonts"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\ClaudeTile"; Filename: "{app}\claudetile.exe"
Name: "{autodesktop}\ClaudeTile"; Filename: "{app}\claudetile.exe"; Tasks: desktopicon

[Registry]
; 和菜单里“开机自启”写的是同一个值，两边开关互认
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ClaudeTile"; ValueData: """{app}\claudetile.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\claudetile.exe"; Description: "{cm:LaunchProgram,ClaudeTile}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM claudetile.exe"; Flags: runhidden; RunOnceId: "Kill"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

function ConfigFile: String;
begin
  Result := ExpandConstant('{userappdata}\ClaudeTile\config.txt');
end;

// 第一次安装：把安装时选的语言写进设置；已经有设置（升级、重装）就不动
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and not FileExists(ConfigFile) then
  begin
    ForceDirectories(ExtractFileDir(ConfigFile));
    SaveStringToFile(ConfigFile, 'lang|' + ActiveLanguage + #13#10, False);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // 菜单里打开的开机自启（安装时没勾也可能有）一并删掉
    RegDeleteValue(HKCU, RunKey, 'ClaudeTile');
    // 静默卸载时按默认“否”，保留数据
    if SuppressibleMsgBox(CustomMessage('DeleteData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
      DelTree(ExpandConstant('{userappdata}\ClaudeTile'), True, True, True);
  end;
end;
