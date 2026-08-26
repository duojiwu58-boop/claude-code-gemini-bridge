#ifndef AppVersion
  #define AppVersion "0.7.2"
#endif
#ifndef SourceDir
  #define SourceDir "..\windows-x64"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{7D3A863F-7F83-4EB9-BAEE-485439C5D7F1}
AppName=Claude Code Multi-Model Bridge
AppVersion={#AppVersion}
AppPublisher=Claude Code Bridge
DefaultDirName={pf64}\ClaudeCodeBridge
DefaultGroupName=Claude Code Multi-Model Bridge
DisableProgramGroupPage=yes
OutputDir={#OutputDir}
OutputBaseFilename=ClaudeCodeBridge-{#AppVersion}-Setup
Compression=lzma2
SolidCompression=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
MinVersion=10.0
SetupLogging=yes
SetupIconFile={#SourceDir}\ClaudeBridgeManager.ico
LicenseFile={#SourceDir}\LICENSE
UninstallDisplayIcon={app}\ClaudeBridgeManager.exe
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Claude Code Multi-Model Bridge Setup

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "快捷方式："; Flags: unchecked

[Files]
Source: "{#SourceDir}\claude-bridge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\ClaudeBridgeManager.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\claude-settings.bridge.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\claude-settings.example.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\install.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\update-api-key.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\uninstall.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\使用说明.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\PROVIDER_CONFIG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.zh-CN.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\examples\providers\*"; DestDir: "{app}\examples\providers"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\scripts\*"; DestDir: "{app}\scripts"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\scripts\stop-bridge.ps1"; Flags: dontcopy

[Icons]
Name: "{group}\模型切换器"; Filename: "{app}\ClaudeBridgeManager.exe"; WorkingDir: "{app}"
Name: "{group}\Provider 配置目录"; Filename: "{sys}\explorer.exe"; Parameters: """{%USERPROFILE}\.claude\bridge-providers"""
Name: "{group}\Provider 配置指南"; Filename: "{app}\PROVIDER_CONFIG.md"; WorkingDir: "{app}"
Name: "{group}\配置或更新 Gemini API Key"; Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\update-api-key.ps1"""; WorkingDir: "{app}"
Name: "{group}\卸载"; Filename: "{uninstallexe}"
Name: "{commondesktop}\Claude Code 模型切换器"; Filename: "{app}\ClaudeBridgeManager.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\ClaudeBridgeManager.exe"; Description: "启动模型切换器"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\uninstall.ps1"" -KeepProgramFiles"; Flags: runhidden waituntilterminated

[Code]
var
  GeminiChoicePage: TInputOptionWizardPage;
  GeminiKeyPage: TInputQueryWizardPage;
  GeminiProxyPage: TInputQueryWizardPage;

function WantsGemini: Boolean;
begin
  Result := GeminiChoicePage.Values[0];
end;

function ExtractProxyValue(const SettingName, ProxySetting: String): String;
var
  LowerSetting: String;
  Marker: String;
  StartPos: Integer;
  EndPos: Integer;
begin
  Result := '';
  LowerSetting := Lowercase(ProxySetting);
  Marker := Lowercase(SettingName) + '=';
  StartPos := Pos(Marker, LowerSetting);
  if StartPos = 0 then
    exit;
  StartPos := StartPos + Length(Marker);
  EndPos := Pos(';', Copy(ProxySetting, StartPos, MaxInt));
  if EndPos = 0 then
    Result := Trim(Copy(ProxySetting, StartPos, MaxInt))
  else
    Result := Trim(Copy(ProxySetting, StartPos, EndPos - 1));
end;

function DetectWindowsProxy: String;
var
  ProxyEnabled: Cardinal;
  ProxySetting: String;
begin
  Result := '';
  if not RegQueryDWordValue(
    HKCU,
    'Software\Microsoft\Windows\CurrentVersion\Internet Settings',
    'ProxyEnable',
    ProxyEnabled
  ) or (ProxyEnabled = 0) then
    exit;
  if not RegQueryStringValue(
    HKCU,
    'Software\Microsoft\Windows\CurrentVersion\Internet Settings',
    'ProxyServer',
    ProxySetting
  ) then
    exit;

  Result := ExtractProxyValue('https', ProxySetting);
  if Result = '' then
    Result := ExtractProxyValue('http', ProxySetting);
  if (Result = '') and (Pos('=', ProxySetting) = 0) then
    Result := Trim(ProxySetting);
  if (Result <> '') and (Pos('://', Result) = 0) then
    Result := 'http://' + Result;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  if (PageID = GeminiKeyPage.ID) or (PageID = GeminiProxyPage.ID) then
    Result := not WantsGemini;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  ProxyValue: String;
begin
  Result := True;
  if (CurPageID = GeminiKeyPage.ID) and
     (Trim(GeminiKeyPage.Values[0]) = '') then
  begin
    MsgBox('请输入 Gemini API Key，或返回上一页取消勾选 Gemini。', mbError, MB_OK);
    Result := False;
    exit;
  end;

  if CurPageID = GeminiProxyPage.ID then
  begin
    ProxyValue := Trim(GeminiProxyPage.Values[0]);
    if (Pos('"', ProxyValue) > 0) or (Pos(#13, ProxyValue) > 0) or
       (Pos(#10, ProxyValue) > 0) then
    begin
      MsgBox('代理地址包含无效字符。', mbError, MB_OK);
      Result := False;
    end
    else if (ProxyValue <> '') and
            (Pos('http://', Lowercase(ProxyValue)) <> 1) and
            (Pos('https://', Lowercase(ProxyValue)) <> 1) then
    begin
      MsgBox('代理地址必须以 http:// 或 https:// 开头。', mbError, MB_OK);
      Result := False;
    end;
  end;
end;

procedure InitializeWizard;
begin
  GeminiChoicePage := CreateInputOptionPage(
    wpSelectTasks,
    '可选模型供应商',
    '是否同时配置 Google Gemini？',
    'Gemini 不是安装本桥接器的必需项。不使用 Gemini 的用户请保持不勾选。',
    False,
    False
  );
  GeminiChoicePage.Add('配置 Google Gemini（需要 Google AI Studio API Key）');
  GeminiChoicePage.Values[0] := False;

  GeminiKeyPage := CreateInputQueryPage(
    GeminiChoicePage.ID,
    'Gemini API Key',
    '输入 Google AI Studio API Key',
    'API Key 仅写入本机受权限保护的配置文件，不会写入服务注册表。'
  );
  GeminiKeyPage.Add('API Key：', True);

  GeminiProxyPage := CreateInputQueryPage(
    GeminiKeyPage.ID,
    'Gemini 网络代理',
    '确认或修改代理地址',
    '已自动检测 Windows 系统代理；可以修改，删除后留空表示直连。'
  );
  GeminiProxyPage.Add('代理地址：', False);
  GeminiProxyPage.Values[0] := DetectWindowsProxy;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Executed: Boolean;
begin
  Result := '';
  ExtractTemporaryFile('stop-bridge.ps1');
  Executed := Exec(
    ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -File "' +
      ExpandConstant('{tmp}\stop-bridge.ps1') + '"',
    ExpandConstant('{tmp}'),
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  );
  if not Executed then
    Result := '无法启动现有服务的停止程序。'
  else if ResultCode <> 0 then
    Result :=
      '无法停止现有 ClaudeCodeBridge 服务，PowerShell 返回代码 ' +
      IntToStr(ResultCode) + '。';
end;

function GetClaudeSettingsDir: String;
var
  UserProfileDir: String;
begin
  UserProfileDir := Trim(GetEnv('USERPROFILE'));
  if UserProfileDir = '' then
    UserProfileDir :=
      ExtractFileDir(ExtractFileDir(ExpandConstant('{userappdata}')));
  Result := AddBackslash(UserProfileDir) + '.claude';
end;

function BuildInstallParameters(const KeyFile: String): String;
begin
  Result :=
    '-NoProfile -ExecutionPolicy Bypass -File "' +
    ExpandConstant('{app}\install.ps1') +
    '" -ClaudeSettingsDir "' +
    GetClaudeSettingsDir +
    '" -NonInteractive -SkipShortcuts';

  if WantsGemini then
  begin
    Result := Result + ' -GeminiMode Configure -ApiKeyFile "' + KeyFile + '"';
    if Trim(GeminiProxyPage.Values[0]) <> '' then
      Result := Result + ' -ProxyUrl "' + Trim(GeminiProxyPage.Values[0]) + '"'
    else
      Result := Result + ' -DirectConnection';
  end
  else
    Result := Result + ' -GeminiMode Skip';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  KeyFile: String;
  ResultCode: Integer;
  Executed: Boolean;
begin
  if CurStep <> ssPostInstall then
    exit;

  KeyFile := ExpandConstant('{tmp}\claude-bridge-gemini-key.txt');
  if WantsGemini then
  begin
    if not SaveStringToFile(KeyFile, Trim(GeminiKeyPage.Values[0]), False) then
      RaiseException('无法创建 API Key 临时文件。');
  end;

  try
    Executed := Exec(
      ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      BuildInstallParameters(KeyFile),
      ExpandConstant('{app}'),
      SW_HIDE,
      ewWaitUntilTerminated,
      ResultCode
    );
    if not Executed then
      RaiseException('无法启动 Windows 服务配置程序。');
    if ResultCode <> 0 then
      RaiseException(
        'Windows 服务配置失败，PowerShell 返回代码 ' +
        IntToStr(ResultCode) + '。'
      );
  finally
    if FileExists(KeyFile) then
      DeleteFile(KeyFile);
  end;
end;
