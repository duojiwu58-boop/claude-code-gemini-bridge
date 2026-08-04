unit BridgeManagerMain;

interface

uses
  Winapi.Windows,
  Winapi.Messages,
  Winapi.CommCtrl,
  Winapi.UxTheme,
  System.SysUtils,
  System.StrUtils,
  System.Classes,
  System.Generics.Collections,
  System.JSON,
  System.UITypes,
  System.Win.Registry,
  System.Net.URLClient,
  System.Net.HttpClient,
  Vcl.Forms,
  Vcl.Controls,
  Vcl.StdCtrls,
  Vcl.ComCtrls,
  Vcl.ExtCtrls,
  Vcl.Graphics,
  Vcl.Dialogs;

type
  TMainForm = class(TForm)
  private
    FHttpClient: THTTPClient;
    FProfileFiles: TStringList;
    FActiveFile: string;
    FBridgeRoot: string;
    FTopPanel: TPanel;
    FAccentPanel: TPanel;
    FStatusPanel: TPanel;
    FStatusDotLabel: TLabel;
    FTitleLabel: TLabel;
    FStatusLabel: TLabel;
    FCurrentModelLabel: TLabel;
    FHintLabel: TLabel;
    FContentPanel: TPanel;
    FListPanel: TPanel;
    FListTitleLabel: TLabel;
    FListHintLabel: TLabel;
    FProfileList: TListView;
    FProxyPanel: TPanel;
    FProxyLabel: TLabel;
    FProxyHintLabel: TLabel;
    FProxyEdit: TEdit;
    FDetectProxyButton: TButton;
    FTestProxyButton: TButton;
    FApplyProxyButton: TButton;
    FLogPanel: TPanel;
    FLogTitleLabel: TLabel;
    FLogMemo: TMemo;
    FStatusBar: TStatusBar;
    FRefreshButton: TButton;
    FSwitchButton: TButton;
    FStartButton: TButton;
    FStopButton: TButton;
    FPollTimer: TTimer;
    procedure FormShown(Sender: TObject);
    procedure FormResize(Sender: TObject);
    procedure RefreshClick(Sender: TObject);
    procedure SwitchClick(Sender: TObject);
    procedure StartClick(Sender: TObject);
    procedure StopClick(Sender: TObject);
    procedure DetectProxyClick(Sender: TObject);
    procedure TestProxyClick(Sender: TObject);
    procedure ApplyProxyClick(Sender: TObject);
    procedure PollTimer(Sender: TObject);
    procedure ProfileDblClick(Sender: TObject);
    procedure BuildUi;
    procedure ApplyWindowsAppearance;
    procedure AppendLog(const AText: string);
    procedure RefreshProfiles;
    procedure RefreshStatus(const AQuiet: Boolean);
    procedure SetBridgeState(const AOnline: Boolean; const ADetail: string);
    procedure SwitchProfile(const AFileName: string);
    procedure UpdateProxyFromStatus(AJson: TJSONObject);
    function ProxyRequestJson: string;
    function DetectWindowsProxy: string;
    function GetJson(const APath: string): TJSONObject;
    function PostJson(const APath, AJson: string): TJSONObject;
    function JsonText(AObject: TJSONObject; const AName: string): string;
    function JsonBool(AObject: TJSONObject; const AName: string): Boolean;
    function FindBridgeRoot: string;
    function RunPowerShellScript(const AScriptPath: string): Cardinal;
  protected
    procedure DoClose(var Action: TCloseAction); override;
  public
    constructor Create(AOwner: TComponent); override;
    destructor Destroy; override;
  end;

var
  MainForm: TMainForm;

implementation

const
  BRIDGE_URL = 'http://127.0.0.1:18787';
  DWMWA_WINDOW_CORNER_PREFERENCE = 33;
  DWMWCP_ROUND = 2;

type
  TDwmSetWindowAttribute = function(
    hWnd: HWND;
    dwAttribute: DWORD;
    pvAttribute: Pointer;
    cbAttribute: DWORD
  ): HRESULT; stdcall;

constructor TMainForm.Create(AOwner: TComponent);
var
  LStage: string;
begin
  LStage := '创建 VCL 窗口';
  try
    inherited CreateNew(AOwner);
    LStage := '设置窗口属性';
    Caption := 'Claude Code 模型切换器';
    Width := 1020;
    Height := 760;
    Position := poScreenCenter;
    Constraints.MinWidth := 900;
    Constraints.MinHeight := 650;
    Color := $00F5F5F5;
    Font.Name := 'Segoe UI';
    Font.Size := 10;
    DoubleBuffered := True;
    Icon.Assign(Application.Icon);

    LStage := '创建 HTTP 客户端';
    FHttpClient := THTTPClient.Create;
    FHttpClient.ConnectionTimeout := 2000;
    FHttpClient.ResponseTimeout := 30000;
    LStage := '创建配置列表';
    FProfileFiles := TStringList.Create;
    FBridgeRoot := FindBridgeRoot;

    LStage := '创建界面控件';
    BuildUi;
    OnShow := FormShown;
    OnResize := FormResize;
  except
    on E: Exception do
      raise Exception.Create(
        'TMainForm.Create/' + LStage + ': ' + E.ClassName + ': ' + E.Message
      );
  end;
end;

destructor TMainForm.Destroy;
begin
  FPollTimer.Enabled := False;
  FHttpClient.Free;
  FProfileFiles.Free;
  inherited;
end;

procedure TMainForm.BuildUi;
var
  LColumn: TListColumn;
begin
  FTopPanel := TPanel.Create(Self);
  FTopPanel.Parent := Self;
  FTopPanel.Align := alTop;
  FTopPanel.Height := 164;
  FTopPanel.BevelOuter := bvNone;
  FTopPanel.Color := clWhite;
  FTopPanel.ParentBackground := False;

  FAccentPanel := TPanel.Create(Self);
  FAccentPanel.Parent := FTopPanel;
  FAccentPanel.Align := alTop;
  FAccentPanel.Height := 4;
  FAccentPanel.BevelOuter := bvNone;
  FAccentPanel.Color := $00D47800;
  FAccentPanel.ParentBackground := False;

  FTitleLabel := TLabel.Create(Self);
  FTitleLabel.Parent := FTopPanel;
  FTitleLabel.SetBounds(24, 20, 430, 34);
  FTitleLabel.Caption := 'Claude Code 模型中心';
  FTitleLabel.Font.Name := 'Segoe UI';
  FTitleLabel.Font.Size := 18;
  FTitleLabel.Font.Style := [fsBold];
  FTitleLabel.Font.Color := $00242424;

  FHintLabel := TLabel.Create(Self);
  FHintLabel.Parent := FTopPanel;
  FHintLabel.SetBounds(25, 57, 500, 22);
  FHintLabel.Caption := '统一管理模型路由、Gemini 代理和桥接服务';
  FHintLabel.Font.Name := 'Segoe UI';
  FHintLabel.Font.Color := $00606060;

  FStatusPanel := TPanel.Create(Self);
  FStatusPanel.Parent := FTopPanel;
  FStatusPanel.SetBounds(24, 91, 430, 56);
  FStatusPanel.BevelOuter := bvNone;
  FStatusPanel.Color := $00F2F2F2;
  FStatusPanel.ParentBackground := False;

  FStatusDotLabel := TLabel.Create(Self);
  FStatusDotLabel.Parent := FStatusPanel;
  FStatusDotLabel.SetBounds(16, 14, 20, 24);
  FStatusDotLabel.Caption := '●';
  FStatusDotLabel.Font.Name := 'Segoe UI Symbol';
  FStatusDotLabel.Font.Size := 11;
  FStatusDotLabel.Font.Color := $00707070;

  FStatusLabel := TLabel.Create(Self);
  FStatusLabel.Parent := FStatusPanel;
  FStatusLabel.SetBounds(43, 8, 360, 22);
  FStatusLabel.Caption := '桥接器：检查中';
  FStatusLabel.Font.Name := 'Segoe UI';
  FStatusLabel.Font.Style := [fsBold];
  FStatusLabel.Font.Color := $00404040;

  FCurrentModelLabel := TLabel.Create(Self);
  FCurrentModelLabel.Parent := FStatusPanel;
  FCurrentModelLabel.SetBounds(43, 30, 360, 20);
  FCurrentModelLabel.Caption := '当前模型：-';
  FCurrentModelLabel.Font.Name := 'Segoe UI';
  FCurrentModelLabel.Font.Color := $00606060;

  FRefreshButton := TButton.Create(Self);
  FRefreshButton.Parent := FTopPanel;
  FRefreshButton.SetBounds(572, 24, 96, 34);
  FRefreshButton.Caption := '刷新配置';
  FRefreshButton.Anchors := [akTop, akRight];
  FRefreshButton.OnClick := RefreshClick;

  FSwitchButton := TButton.Create(Self);
  FSwitchButton.Parent := FTopPanel;
  FSwitchButton.SetBounds(676, 24, 132, 34);
  FSwitchButton.Caption := '切换选中模型';
  FSwitchButton.Anchors := [akTop, akRight];
  FSwitchButton.Default := True;
  FSwitchButton.OnClick := SwitchClick;

  FStartButton := TButton.Create(Self);
  FStartButton.Parent := FTopPanel;
  FStartButton.SetBounds(816, 24, 86, 34);
  FStartButton.Caption := '启动服务';
  FStartButton.Anchors := [akTop, akRight];
  FStartButton.OnClick := StartClick;

  FStopButton := TButton.Create(Self);
  FStopButton.Parent := FTopPanel;
  FStopButton.SetBounds(910, 24, 86, 34);
  FStopButton.Caption := '停止服务';
  FStopButton.Anchors := [akTop, akRight];
  FStopButton.OnClick := StopClick;

  FStatusBar := TStatusBar.Create(Self);
  FStatusBar.Parent := Self;
  FStatusBar.Align := alBottom;
  FStatusBar.Height := 28;
  FStatusBar.SimplePanel := True;
  FStatusBar.SimpleText := '配置目录：%USERPROFILE%\.claude';
  FStatusBar.Font.Name := 'Segoe UI';

  FContentPanel := TPanel.Create(Self);
  FContentPanel.Parent := Self;
  FContentPanel.Align := alClient;
  FContentPanel.BevelOuter := bvNone;
  FContentPanel.Color := $00F5F5F5;
  FContentPanel.ParentBackground := False;
  FContentPanel.Padding.SetBounds(16, 14, 16, 14);

  FProxyPanel := TPanel.Create(Self);
  FProxyPanel.Parent := FContentPanel;
  FProxyPanel.Align := alBottom;
  FProxyPanel.AlignWithMargins := True;
  FProxyPanel.Margins.SetBounds(0, 12, 0, 0);
  FProxyPanel.Height := 100;
  FProxyPanel.BevelOuter := bvNone;
  FProxyPanel.Color := clWhite;
  FProxyPanel.ParentBackground := False;

  FProxyHintLabel := TLabel.Create(Self);
  FProxyHintLabel.Parent := FProxyPanel;
  FProxyHintLabel.SetBounds(16, 12, 560, 22);
  FProxyHintLabel.Caption :=
    'Gemini 网络代理  ·  留空保存表示直连';
  FProxyHintLabel.Font.Name := 'Segoe UI';
  FProxyHintLabel.Font.Style := [fsBold];
  FProxyHintLabel.Font.Color := $00303030;

  FProxyLabel := TLabel.Create(Self);
  FProxyLabel.Parent := FProxyPanel;
  FProxyLabel.SetBounds(16, 50, 112, 20);
  FProxyLabel.Caption := '代理地址';
  FProxyLabel.Font.Name := 'Segoe UI';
  FProxyLabel.Font.Color := $00505050;

  FProxyEdit := TEdit.Create(Self);
  FProxyEdit.Parent := FProxyPanel;
  FProxyEdit.SetBounds(104, 44, 448, 28);
  FProxyEdit.Anchors := [akLeft, akTop, akRight];
  FProxyEdit.Font.Name := 'Segoe UI';
  FProxyEdit.TextHint := '例如 http://127.0.0.1:8080';

  FDetectProxyButton := TButton.Create(Self);
  FDetectProxyButton.Parent := FProxyPanel;
  FDetectProxyButton.SetBounds(566, 42, 118, 32);
  FDetectProxyButton.Anchors := [akTop, akRight];
  FDetectProxyButton.Caption := '检测系统代理';
  FDetectProxyButton.OnClick := DetectProxyClick;

  FTestProxyButton := TButton.Create(Self);
  FTestProxyButton.Parent := FProxyPanel;
  FTestProxyButton.SetBounds(692, 42, 100, 32);
  FTestProxyButton.Anchors := [akTop, akRight];
  FTestProxyButton.Caption := '测试连接';
  FTestProxyButton.OnClick := TestProxyClick;

  FApplyProxyButton := TButton.Create(Self);
  FApplyProxyButton.Parent := FProxyPanel;
  FApplyProxyButton.SetBounds(800, 42, 156, 32);
  FApplyProxyButton.Anchors := [akTop, akRight];
  FApplyProxyButton.Caption := '保存并立即生效';
  FApplyProxyButton.OnClick := ApplyProxyClick;

  FLogPanel := TPanel.Create(Self);
  FLogPanel.Parent := FContentPanel;
  FLogPanel.Align := alBottom;
  FLogPanel.AlignWithMargins := True;
  FLogPanel.Margins.SetBounds(0, 12, 0, 0);
  FLogPanel.Height := 126;
  FLogPanel.BevelOuter := bvNone;
  FLogPanel.Color := clWhite;
  FLogPanel.ParentBackground := False;
  FLogPanel.Padding.SetBounds(16, 40, 16, 12);

  FLogTitleLabel := TLabel.Create(Self);
  FLogTitleLabel.Parent := FLogPanel;
  FLogTitleLabel.SetBounds(16, 12, 300, 22);
  FLogTitleLabel.Caption := '运行记录';
  FLogTitleLabel.Font.Name := 'Segoe UI';
  FLogTitleLabel.Font.Style := [fsBold];
  FLogTitleLabel.Font.Color := $00303030;

  FLogMemo := TMemo.Create(Self);
  FLogMemo.Parent := FLogPanel;
  FLogMemo.Align := alClient;
  FLogMemo.BorderStyle := bsNone;
  FLogMemo.Color := $00FAFAFA;
  FLogMemo.ReadOnly := True;
  FLogMemo.ScrollBars := ssVertical;
  FLogMemo.Font.Name := 'Cascadia Mono';
  FLogMemo.Font.Size := 9;

  FListPanel := TPanel.Create(Self);
  FListPanel.Parent := FContentPanel;
  FListPanel.Align := alClient;
  FListPanel.BevelOuter := bvNone;
  FListPanel.Color := clWhite;
  FListPanel.ParentBackground := False;
  FListPanel.Padding.SetBounds(16, 52, 16, 12);

  FListTitleLabel := TLabel.Create(Self);
  FListTitleLabel.Parent := FListPanel;
  FListTitleLabel.SetBounds(16, 12, 220, 24);
  FListTitleLabel.Caption := '可用模型';
  FListTitleLabel.Font.Name := 'Segoe UI';
  FListTitleLabel.Font.Size := 11;
  FListTitleLabel.Font.Style := [fsBold];
  FListTitleLabel.Font.Color := $00242424;

  FListHintLabel := TLabel.Create(Self);
  FListHintLabel.Parent := FListPanel;
  FListHintLabel.SetBounds(126, 15, 520, 20);
  FListHintLabel.Caption := '双击模型即可切换，下一个请求立即生效';
  FListHintLabel.Font.Name := 'Segoe UI';
  FListHintLabel.Font.Color := $00606060;

  FProfileList := TListView.Create(Self);
  FProfileList.Parent := FListPanel;
  FProfileList.Align := alClient;
  FProfileList.ViewStyle := vsReport;
  FProfileList.BorderStyle := bsNone;
  FProfileList.ReadOnly := True;
  FProfileList.RowSelect := True;
  FProfileList.HideSelection := False;
  FProfileList.GridLines := False;
  FProfileList.OnDblClick := ProfileDblClick;
  FProfileList.Font.Name := 'Segoe UI';
  FProfileList.Font.Size := 10;

  LColumn := FProfileList.Columns.Add;
  LColumn.Caption := '模型';
  LColumn.Width := 205;
  LColumn := FProfileList.Columns.Add;
  LColumn.Caption := '服务地址';
  LColumn.Width := 285;
  LColumn := FProfileList.Columns.Add;
  LColumn.Caption := '代理';
  LColumn.Width := 140;
  LColumn := FProfileList.Columns.Add;
  LColumn.Caption := '路由';
  LColumn.Width := 105;
  LColumn := FProfileList.Columns.Add;
  LColumn.Caption := '配置文件';
  LColumn.Width := 165;

  FPollTimer := TTimer.Create(Self);
  FPollTimer.Interval := 3000;
  FPollTimer.Enabled := False;
  FPollTimer.OnTimer := PollTimer;
end;

procedure TMainForm.FormShown(Sender: TObject);
var
  LStage: string;
begin
  LStage := '记录桥接器目录';
  try
    FormResize(Self);
    ApplyWindowsAppearance;
    AppendLog('桥接器目录：' + FBridgeRoot);
    LStage := '读取桥接器状态';
    RefreshStatus(True);
    LStage := '读取模型配置列表';
    if FStatusLabel.Tag = 1 then
      RefreshProfiles;
    LStage := '启动状态轮询';
    FPollTimer.Enabled := True;
  except
    on E: Exception do
    begin
      MessageDlg(
        'GUI 初始化失败：' + LStage + sLineBreak + E.ClassName + ': ' + E.Message,
        mtError,
        [mbOK],
        0
      );
    end;
  end;
end;

procedure TMainForm.FormResize(Sender: TObject);
const
  CONTROL_GAP = 8;
  EDGE_MARGIN = 24;
  PROXY_EDGE_MARGIN = 16;
begin
  if Assigned(FStopButton) and Assigned(FTopPanel) then
  begin
    FStopButton.Left :=
      FTopPanel.ClientWidth - EDGE_MARGIN - FStopButton.Width;
    FStartButton.Left :=
      FStopButton.Left - CONTROL_GAP - FStartButton.Width;
    FSwitchButton.Left :=
      FStartButton.Left - CONTROL_GAP - FSwitchButton.Width;
    FRefreshButton.Left :=
      FSwitchButton.Left - CONTROL_GAP - FRefreshButton.Width;
  end;

  if Assigned(FApplyProxyButton) and Assigned(FProxyPanel) then
  begin
    FApplyProxyButton.Left :=
      FProxyPanel.ClientWidth - PROXY_EDGE_MARGIN - FApplyProxyButton.Width;
    FTestProxyButton.Left :=
      FApplyProxyButton.Left - CONTROL_GAP - FTestProxyButton.Width;
    FDetectProxyButton.Left :=
      FTestProxyButton.Left - CONTROL_GAP - FDetectProxyButton.Width;
    FProxyEdit.Width :=
      FDetectProxyButton.Left - CONTROL_GAP - FProxyEdit.Left;
  end;
end;

procedure TMainForm.ApplyWindowsAppearance;
var
  LDwmApi: HMODULE;
  LDwmSetWindowAttribute: TDwmSetWindowAttribute;
  LCornerPreference: Integer;
begin
  SetWindowTheme(FProfileList.Handle, 'Explorer', nil);
  SetWindowTheme(FProxyEdit.Handle, 'Explorer', nil);
  SendMessage(
    FProfileList.Handle,
    LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVS_EX_DOUBLEBUFFER,
    LVS_EX_DOUBLEBUFFER
  );

  LDwmApi := LoadLibrary('dwmapi.dll');
  if LDwmApi = 0 then
    Exit;
  try
    LDwmSetWindowAttribute := TDwmSetWindowAttribute(
      GetProcAddress(LDwmApi, PAnsiChar(AnsiString('DwmSetWindowAttribute')))
    );
    if Assigned(LDwmSetWindowAttribute) then
    begin
      LCornerPreference := DWMWCP_ROUND;
      LDwmSetWindowAttribute(
        Handle,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        @LCornerPreference,
        SizeOf(LCornerPreference)
      );
    end;
  finally
    FreeLibrary(LDwmApi);
  end;
end;

procedure TMainForm.DoClose(var Action: TCloseAction);
begin
  FPollTimer.Enabled := False;
  Action := caFree;
  Application.Terminate;
end;

procedure TMainForm.AppendLog(const AText: string);
begin
  FLogMemo.Lines.Add(FormatDateTime('hh:nn:ss', Now) + '  ' + AText);
  FLogMemo.SelStart := Length(FLogMemo.Text);
end;

function TMainForm.JsonText(AObject: TJSONObject; const AName: string): string;
var
  LValue: TJSONValue;
begin
  Result := '';
  if not Assigned(AObject) then
    Exit;
  LValue := AObject.Values[AName];
  if not Assigned(LValue) or (LValue is TJSONNull) then
    Exit;
  Result := LValue.Value;
end;

function TMainForm.JsonBool(AObject: TJSONObject; const AName: string): Boolean;
begin
  Result := SameText(JsonText(AObject, AName), 'true');
end;

procedure TMainForm.UpdateProxyFromStatus(AJson: TJSONObject);
var
  LProxy: string;
begin
  LProxy := JsonText(AJson, 'gemini_proxy');
  if not FProxyEdit.Focused then
    FProxyEdit.Text := LProxy;
end;

function TMainForm.ProxyRequestJson: string;
var
  LRequest: TJSONObject;
  LProxy: string;
begin
  LRequest := TJSONObject.Create;
  try
    LProxy := Trim(FProxyEdit.Text);
    if LProxy = '' then
      LRequest.AddPair('proxy', TJSONNull.Create)
    else
      LRequest.AddPair('proxy', LProxy);
    Result := LRequest.ToJSON;
  finally
    LRequest.Free;
  end;
end;

function TMainForm.DetectWindowsProxy: string;
var
  LRegistry: TRegistry;
  LProxyServer: string;
  LParts: TStringList;
  LPart: string;
  LHttpProxy: string;
  I: Integer;
begin
  Result := '';
  LRegistry := TRegistry.Create(KEY_READ);
  try
    LRegistry.RootKey := HKEY_CURRENT_USER;
    if not LRegistry.OpenKeyReadOnly(
      '\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
    ) then
      Exit;
    if not LRegistry.ValueExists('ProxyEnable') or
       (LRegistry.ReadInteger('ProxyEnable') = 0) or
       not LRegistry.ValueExists('ProxyServer') then
      Exit;
    LProxyServer := Trim(LRegistry.ReadString('ProxyServer'));
  finally
    LRegistry.Free;
  end;
  if LProxyServer = '' then
    Exit;

  LParts := TStringList.Create;
  try
    LParts.StrictDelimiter := True;
    LParts.Delimiter := ';';
    LParts.DelimitedText := LProxyServer;
    for I := 0 to LParts.Count - 1 do
    begin
      LPart := Trim(LParts[I]);
      if StartsText('https=', LPart) then
      begin
        Result := Trim(Copy(LPart, 7, MaxInt));
        Break;
      end;
      if StartsText('http=', LPart) then
        LHttpProxy := Trim(Copy(LPart, 6, MaxInt));
    end;
    if Result = '' then
      Result := LHttpProxy;
    if (Result = '') and (LParts.Count = 1) then
      Result := Trim(LParts[0]);
  finally
    LParts.Free;
  end;
  if (Result <> '') and (Pos('://', Result) = 0) then
    Result := 'http://' + Result;
end;

function TMainForm.GetJson(const APath: string): TJSONObject;
var
  LResponse: IHTTPResponse;
  LValue: TJSONValue;
begin
  LResponse := FHttpClient.Get(BRIDGE_URL + APath);
  if LResponse.StatusCode <> 200 then
    raise Exception.CreateFmt('HTTP %d: %s', [
      LResponse.StatusCode,
      LResponse.ContentAsString(TEncoding.UTF8)
    ]);
  LValue := TJSONObject.ParseJSONValue(
    LResponse.ContentAsString(TEncoding.UTF8)
  );
  if not (LValue is TJSONObject) then
  begin
    LValue.Free;
    raise Exception.Create('桥接器返回了无效 JSON');
  end;
  Result := TJSONObject(LValue);
end;

function TMainForm.PostJson(const APath, AJson: string): TJSONObject;
var
  LHeaders: TNetHeaders;
  LResponse: IHTTPResponse;
  LStream: TStringStream;
  LValue: TJSONValue;
begin
  SetLength(LHeaders, 1);
  LHeaders[0].Name := 'Content-Type';
  LHeaders[0].Value := 'application/json';
  LStream := TStringStream.Create(AJson, TEncoding.UTF8);
  try
    LResponse := FHttpClient.Post(
      BRIDGE_URL + APath,
      LStream,
      nil,
      LHeaders
    );
  finally
    LStream.Free;
  end;
  if LResponse.StatusCode <> 200 then
    raise Exception.CreateFmt('HTTP %d: %s', [
      LResponse.StatusCode,
      LResponse.ContentAsString(TEncoding.UTF8)
    ]);
  LValue := TJSONObject.ParseJSONValue(
    LResponse.ContentAsString(TEncoding.UTF8)
  );
  if not (LValue is TJSONObject) then
  begin
    LValue.Free;
    raise Exception.Create('桥接器返回了无效 JSON');
  end;
  Result := TJSONObject(LValue);
end;

procedure TMainForm.SetBridgeState(
  const AOnline: Boolean;
  const ADetail: string
);
begin
  if AOnline then
  begin
    FStatusLabel.Caption := '桥接器：运行中';
    FStatusLabel.Font.Color := $00106410;
    FStatusDotLabel.Font.Color := $00107C10;
    FStatusPanel.Color := $00ECF6E9;
    FStatusLabel.Tag := 1;
    FSwitchButton.Enabled := True;
    FStopButton.Enabled := True;
    FStartButton.Enabled := False;
    FTestProxyButton.Enabled := True;
    FApplyProxyButton.Enabled := True;
  end
  else
  begin
    FStatusLabel.Caption := '桥接器：未运行';
    FStatusLabel.Font.Color := $001C2BC4;
    FStatusDotLabel.Font.Color := $001C2BC4;
    FStatusPanel.Color := $00EDEFFB;
    FStatusLabel.Tag := 0;
    FCurrentModelLabel.Caption := '当前模型：-';
    FSwitchButton.Enabled := False;
    FStopButton.Enabled := False;
    FStartButton.Enabled := True;
    FTestProxyButton.Enabled := False;
    FApplyProxyButton.Enabled := False;
  end;
  FStatusBar.SimpleText := ADetail;
end;

procedure TMainForm.RefreshStatus(const AQuiet: Boolean);
var
  LJson: TJSONObject;
  LProfile: TJSONObject;
  LModel: string;
  LFileName: string;
begin
  try
    LJson := GetJson('/admin/status');
    try
      UpdateProxyFromStatus(LJson);
      LProfile := nil;
      if LJson.Values['active_profile'] is TJSONObject then
        LProfile := TJSONObject(LJson.Values['active_profile']);
      if Assigned(LProfile) then
      begin
        LModel := JsonText(LProfile, 'model');
        LFileName := JsonText(LProfile, 'file');
        FActiveFile := LFileName;
        FCurrentModelLabel.Caption := '当前模型：' + LModel;
        SetBridgeState(True, '当前配置：' + LFileName);
      end
      else
      begin
        FActiveFile := '';
        FCurrentModelLabel.Caption := '当前模型：暂无可用配置';
        SetBridgeState(True, '桥接器运行中，请添加 settings - *.json 模型配置');
        FSwitchButton.Enabled := False;
      end;
    finally
      LJson.Free;
    end;
  except
    on E: Exception do
    begin
      SetBridgeState(False, E.Message);
      if not AQuiet then
        AppendLog('状态检查失败：' + E.Message);
    end;
  end;
end;

procedure TMainForm.RefreshProfiles;
var
  LReload: TJSONObject;
  LJson: TJSONObject;
  LArray: TJSONArray;
  LProfile: TJSONObject;
  LItem: TListItem;
  LModel: string;
  LFileName: string;
  LBaseUrl: string;
  LProxy: string;
  LRoute: string;
  I: Integer;
begin
  LReload := PostJson('/admin/reload-profiles', '{}');
  LReload.Free;
  LJson := GetJson('/admin/profiles');
  try
    FProfileList.Items.BeginUpdate;
    try
      FProfileList.Items.Clear;
      FProfileFiles.Clear;
      LArray := LJson.Values['profiles'] as TJSONArray;
      for I := 0 to LArray.Count - 1 do
      begin
        LProfile := LArray.Items[I] as TJSONObject;
        LModel := JsonText(LProfile, 'model');
        LFileName := JsonText(LProfile, 'file');
        LBaseUrl := JsonText(LProfile, 'base_url');
        LProxy := JsonText(LProfile, 'proxy');
        if JsonBool(LProfile, 'local_gemini') then
        begin
          LRoute := 'Gemini 转换';
          LProxy := Trim(FProxyEdit.Text);
        end
        else
          LRoute := 'Anthropic 直通';
        if LProxy = '' then
          LProxy := '(直连)';

        LItem := FProfileList.Items.Add;
        if JsonBool(LProfile, 'active') then
        begin
          LItem.Caption := '✓  ' + LModel;
          LItem.Selected := True;
          LItem.Focused := True;
          FActiveFile := LFileName;
        end
        else
          LItem.Caption := LModel;
        LItem.SubItems.Add(LBaseUrl);
        LItem.SubItems.Add(LProxy);
        LItem.SubItems.Add(LRoute);
        LItem.SubItems.Add(LFileName);
        FProfileFiles.Add(LFileName);
      end;
    finally
      FProfileList.Items.EndUpdate;
    end;
    AppendLog(Format('已加载 %d 个模型配置。', [FProfileFiles.Count]));
  finally
    LJson.Free;
  end;
  RefreshStatus(True);
end;

procedure TMainForm.SwitchProfile(const AFileName: string);
var
  LRequest: TJSONObject;
  LResult: TJSONObject;
begin
  LRequest := TJSONObject.Create;
  try
    LRequest.AddPair('file', AFileName);
    LResult := PostJson('/admin/active-profile', LRequest.ToJSON);
    try
      AppendLog('模型已切换：' + AFileName);
    finally
      LResult.Free;
    end;
  finally
    LRequest.Free;
  end;
  RefreshProfiles;
end;

procedure TMainForm.RefreshClick(Sender: TObject);
begin
  try
    RefreshProfiles;
  except
    on E: Exception do
    begin
      AppendLog('刷新失败：' + E.Message);
      MessageDlg(E.Message, mtError, [mbOK], 0);
    end;
  end;
end;

procedure TMainForm.SwitchClick(Sender: TObject);
var
  LIndex: Integer;
begin
  if not Assigned(FProfileList.Selected) then
  begin
    MessageDlg('请先选择一个模型配置。', mtInformation, [mbOK], 0);
    Exit;
  end;
  LIndex := FProfileList.Selected.Index;
  if (LIndex < 0) or (LIndex >= FProfileFiles.Count) then
    Exit;
  try
    SwitchProfile(FProfileFiles[LIndex]);
  except
    on E: Exception do
    begin
      AppendLog('切换失败：' + E.Message);
      MessageDlg(E.Message, mtError, [mbOK], 0);
    end;
  end;
end;

procedure TMainForm.ProfileDblClick(Sender: TObject);
begin
  SwitchClick(Sender);
end;

function TMainForm.FindBridgeRoot: string;
var
  I: Integer;
begin
  Result := ExcludeTrailingPathDelimiter(ExtractFilePath(ParamStr(0)));
  for I := 0 to 5 do
  begin
    if FileExists(Result + '\scripts\start-bridge.ps1') then
      Exit;
    Result := ExtractFileDir(Result);
  end;
  Result := ExcludeTrailingPathDelimiter(ExtractFilePath(ParamStr(0)));
end;

function TMainForm.RunPowerShellScript(const AScriptPath: string): Cardinal;
var
  LCommandLine: string;
  LPowerShell: string;
  LStartupInfo: TStartupInfo;
  LProcessInfo: TProcessInformation;
begin
  LPowerShell :=
    GetEnvironmentVariable('SystemRoot') +
    '\System32\WindowsPowerShell\v1.0\powershell.exe';
  LCommandLine := Format(
    '"%s" -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%s"',
    [LPowerShell, AScriptPath]
  );
  UniqueString(LCommandLine);
  ZeroMemory(@LStartupInfo, SizeOf(LStartupInfo));
  ZeroMemory(@LProcessInfo, SizeOf(LProcessInfo));
  LStartupInfo.cb := SizeOf(LStartupInfo);
  LStartupInfo.dwFlags := STARTF_USESHOWWINDOW;
  LStartupInfo.wShowWindow := SW_HIDE;

  if not CreateProcess(
    nil,
    PChar(LCommandLine),
    nil,
    nil,
    False,
    CREATE_NO_WINDOW,
    nil,
    PChar(FBridgeRoot),
    LStartupInfo,
    LProcessInfo
  ) then
    RaiseLastOSError;
  try
    WaitForSingleObject(LProcessInfo.hProcess, 30000);
    if not GetExitCodeProcess(LProcessInfo.hProcess, Result) then
      RaiseLastOSError;
  finally
    CloseHandle(LProcessInfo.hThread);
    CloseHandle(LProcessInfo.hProcess);
  end;
end;

procedure TMainForm.StartClick(Sender: TObject);
var
  LExitCode: Cardinal;
begin
  FPollTimer.Enabled := False;
  try
    LExitCode := RunPowerShellScript(
      FBridgeRoot + '\scripts\start-bridge.ps1'
    );
    if LExitCode <> 0 then
      raise Exception.CreateFmt('启动脚本返回错误码 %d', [LExitCode]);
    AppendLog('桥接器已启动。');
    Sleep(500);
    RefreshStatus(False);
    if FStatusLabel.Tag = 1 then
      RefreshProfiles;
  except
    on E: Exception do
    begin
      AppendLog('启动失败：' + E.Message);
      MessageDlg(E.Message, mtError, [mbOK], 0);
    end;
  end;
  FPollTimer.Enabled := True;
end;

procedure TMainForm.StopClick(Sender: TObject);
var
  LExitCode: Cardinal;
begin
  FPollTimer.Enabled := False;
  try
    LExitCode := RunPowerShellScript(
      FBridgeRoot + '\scripts\stop-bridge.ps1'
    );
    if LExitCode <> 0 then
      raise Exception.CreateFmt('停止脚本返回错误码 %d', [LExitCode]);
    AppendLog('桥接器已停止。');
    SetBridgeState(False, '桥接器已停止');
  except
    on E: Exception do
    begin
      AppendLog('停止失败：' + E.Message);
      MessageDlg(E.Message, mtError, [mbOK], 0);
    end;
  end;
  FPollTimer.Enabled := True;
end;

procedure TMainForm.DetectProxyClick(Sender: TObject);
var
  LProxy: string;
begin
  LProxy := DetectWindowsProxy;
  if LProxy = '' then
  begin
    AppendLog('未检测到已启用的 Windows 系统代理。');
    MessageDlg(
      '未检测到已启用的 Windows 系统代理。',
      mtInformation,
      [mbOK],
      0
    );
    Exit;
  end;
  FProxyEdit.Text := LProxy;
  AppendLog('已检测到系统代理：' + LProxy);
end;

procedure TMainForm.TestProxyClick(Sender: TObject);
var
  LResult: TJSONObject;
begin
  FTestProxyButton.Enabled := False;
  try
    LResult := PostJson('/admin/gemini-proxy/test', ProxyRequestJson);
    try
      AppendLog(
        'Gemini 连接测试成功，模型：' + JsonText(LResult, 'model')
      );
      MessageDlg('Gemini 连接测试成功。', mtInformation, [mbOK], 0);
    finally
      LResult.Free;
    end;
  except
    on E: Exception do
    begin
      AppendLog('Gemini 连接测试失败：' + E.Message);
      MessageDlg(E.Message, mtError, [mbOK], 0);
    end;
  end;
  FTestProxyButton.Enabled := FStatusLabel.Tag = 1;
end;

procedure TMainForm.ApplyProxyClick(Sender: TObject);
var
  LResult: TJSONObject;
  LProxy: string;
begin
  FApplyProxyButton.Enabled := False;
  try
    LResult := PostJson('/admin/gemini-proxy', ProxyRequestJson);
    try
      LProxy := JsonText(LResult, 'gemini_proxy');
      if LProxy = '' then
        AppendLog('Gemini 已切换为直连。')
      else
        AppendLog('Gemini 代理已保存并立即生效：' + LProxy);
    finally
      LResult.Free;
    end;
    RefreshProfiles;
  except
    on E: Exception do
    begin
      AppendLog('保存 Gemini 代理失败：' + E.Message);
      MessageDlg(E.Message, mtError, [mbOK], 0);
    end;
  end;
  FApplyProxyButton.Enabled := FStatusLabel.Tag = 1;
end;

procedure TMainForm.PollTimer(Sender: TObject);
begin
  RefreshStatus(True);
end;

end.
