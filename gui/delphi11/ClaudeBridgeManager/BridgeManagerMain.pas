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
  System.Math,
  System.IOUtils,
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
  TProfileRow = record
    Model: string;
    BaseUrl: string;
    Proxy: string;
    Route: string;
    FileName: string;
    Active: Boolean;
  end;

  TStatusSnapshot = record
    Online: Boolean;
    HasProfile: Boolean;
    Model: string;
    FileName: string;
    Proxy: string;
    Stamp: string;
    SettingsDir: string;
    Detail: string;
  end;

  TMainForm = class(TForm)
  private
    FBridgeUrl: string;
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
    FClosing: Boolean;
    FBusy: Boolean;
    FStatusInFlight: Boolean;
    FProfilesBusy: Boolean;
    FProfilesLoaded: Boolean;
    FProxyDirty: Boolean;
    FUpdatingProxy: Boolean;
    FSettingsStamp: string;
    FWorkerCount: Integer;
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
    procedure ListSelectItem(Sender: TObject; Item: TListItem;
      Selected: Boolean);
    procedure ProxyEditChange(Sender: TObject);
    procedure BuildUi;
    procedure ApplyWindowsAppearance;
    procedure AppendLog(const AText: string);
    procedure QueueToMain(const AProc: TProc);
    procedure SetBusy(const ABusy: Boolean);
    procedure RefreshStatusAsync(const AQuiet: Boolean);
    procedure StartProfilesRefresh(const AQuiet: Boolean);
    procedure SwitchProfileAsync(const AFileName: string);
    procedure StartStopAsync(const AStart: Boolean);
    procedure ApplyStatusSnapshot(const ASnapshot: TStatusSnapshot;
      const AQuiet: Boolean);
    procedure FillProfileList(const ARows: TArray<TProfileRow>);
    procedure AdjustProfileColumns;
    procedure SetBridgeState(const AOnline: Boolean; const ADetail: string);
    procedure UpdateProxyFromStatus(const AServerProxy: string);
    function ProxyRequestJson: string;
    function DetectWindowsProxy: string;
    function NewHttpClient(const AResponseTimeout: Integer): THTTPClient;
    function GetJsonWith(AClient: THTTPClient; const APath: string): TJSONObject;
    function PostJsonWith(AClient: THTTPClient; const APath,
      AJson: string): TJSONObject;
    function JsonText(AObject: TJSONObject; const AName: string): string;
    function JsonBool(AObject: TJSONObject; const AName: string): Boolean;
    function FindBridgeRoot: string;
    function ResolveBridgeUrl: string;
    function ReadStateListen(const AStatePath: string): string;
    function ReadServiceStateFilePath: string;
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
  DEFAULT_BRIDGE_URL = 'http://127.0.0.1:18787';
  STATUS_RESPONSE_TIMEOUT_MS = 4000;
  ACTION_RESPONSE_TIMEOUT_MS = 30000;
  LOG_MAX_LINES = 500;
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
    Caption := 'Claude Code 模型中心';
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

    LStage := '创建配置列表';
    FProfileFiles := TStringList.Create;
    FBridgeRoot := FindBridgeRoot;
    FBridgeUrl := ResolveBridgeUrl;

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
  FRefreshButton.OnClick := RefreshClick;

  FSwitchButton := TButton.Create(Self);
  FSwitchButton.Parent := FTopPanel;
  FSwitchButton.SetBounds(676, 24, 132, 34);
  FSwitchButton.Caption := '切换选中模型';
  FSwitchButton.Default := True;
  FSwitchButton.OnClick := SwitchClick;

  FStartButton := TButton.Create(Self);
  FStartButton.Parent := FTopPanel;
  FStartButton.SetBounds(816, 24, 86, 34);
  FStartButton.Caption := '启动服务';
  FStartButton.OnClick := StartClick;

  FStopButton := TButton.Create(Self);
  FStopButton.Parent := FTopPanel;
  FStopButton.SetBounds(910, 24, 86, 34);
  FStopButton.Caption := '停止服务';
  FStopButton.OnClick := StopClick;

  FStatusBar := TStatusBar.Create(Self);
  FStatusBar.Parent := Self;
  FStatusBar.Align := alBottom;
  FStatusBar.Height := 28;
  FStatusBar.SimplePanel := False;
  FStatusBar.Panels.Add;
  FStatusBar.Panels.Add;
  FStatusBar.Panels.Add;
  FStatusBar.Panels[0].Text := '桥接地址：' + FBridgeUrl;
  FStatusBar.Panels[1].Text :=
    '配置目录：%USERPROFILE%\.claude\bridge-providers';
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
  FProxyEdit.Font.Name := 'Segoe UI';
  FProxyEdit.TextHint := '例如 http://127.0.0.1:8080';
  FProxyEdit.OnChange := ProxyEditChange;

  FDetectProxyButton := TButton.Create(Self);
  FDetectProxyButton.Parent := FProxyPanel;
  FDetectProxyButton.SetBounds(566, 42, 118, 32);
  FDetectProxyButton.Caption := '检测系统代理';
  FDetectProxyButton.OnClick := DetectProxyClick;

  FTestProxyButton := TButton.Create(Self);
  FTestProxyButton.Parent := FProxyPanel;
  FTestProxyButton.SetBounds(692, 42, 100, 32);
  FTestProxyButton.Caption := '测试连接';
  FTestProxyButton.OnClick := TestProxyClick;

  FApplyProxyButton := TButton.Create(Self);
  FApplyProxyButton.Parent := FProxyPanel;
  FApplyProxyButton.SetBounds(800, 42, 156, 32);
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
  FProfileList.OnSelectItem := ListSelectItem;
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
    AppendLog('桥接地址：' + FBridgeUrl);
    LStage := '读取桥接器状态';
    RefreshStatusAsync(False);
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

  if Assigned(FStatusBar) then
  begin
    FStatusBar.Panels[0].Width := FStatusBar.ClientWidth * 2 div 5;
    FStatusBar.Panels[1].Width := FStatusBar.ClientWidth * 3 div 10;
  end;

  AdjustProfileColumns;
end;

procedure TMainForm.AdjustProfileColumns;
var
  LTotal: Integer;
  LModel: Integer;
  LUrl: Integer;
  LProxy: Integer;
  LRoute: Integer;
begin
  if not Assigned(FProfileList) or not FProfileList.HandleAllocated then
    Exit;
  LTotal := FProfileList.ClientWidth - GetSystemMetrics(SM_CXVSCROLL) - 4;
  if LTotal < 500 then
    Exit;
  LModel := Max(170, LTotal * 22 div 100);
  LUrl := Max(230, LTotal * 30 div 100);
  LProxy := Max(80, LTotal * 10 div 100);
  LRoute := Max(100, LTotal * 12 div 100);
  FProfileList.Columns[0].Width := LModel;
  FProfileList.Columns[1].Width := LUrl;
  FProfileList.Columns[2].Width := LProxy;
  FProfileList.Columns[3].Width := LRoute;
  FProfileList.Columns[4].Width :=
    Max(140, LTotal - LModel - LUrl - LProxy - LRoute);
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
  FClosing := True;
  FPollTimer.Enabled := False;
  if FWorkerCount > 0 then
  begin
    Enabled := False;
    FStatusBar.Panels[0].Text := '正在等待后台操作结束...';
    Action := caNone;
    Exit;
  end;
  Action := caFree;
  Application.Terminate;
end;

procedure TMainForm.AppendLog(const AText: string);
begin
  FLogMemo.Lines.BeginUpdate;
  try
    FLogMemo.Lines.Add(FormatDateTime('hh:nn:ss', Now) + '  ' + AText);
    while FLogMemo.Lines.Count > LOG_MAX_LINES do
      FLogMemo.Lines.Delete(0);
    FLogMemo.SelStart := Length(FLogMemo.Text);
    FLogMemo.Perform(EM_SCROLLCARET, 0, 0);
  finally
    FLogMemo.Lines.EndUpdate;
  end;
end;

procedure TMainForm.QueueToMain(const AProc: TProc);
var
  LQueued: TThreadProcedure;
begin
  LQueued :=
    procedure
    begin
      try
        if not FClosing then
          AProc();
      finally
        if FWorkerCount > 0 then
          Dec(FWorkerCount);
        if FClosing and (FWorkerCount = 0) and HandleAllocated then
          PostMessage(Handle, WM_CLOSE, 0, 0);
      end;
    end;
  TThread.Queue(nil, LQueued);
end;

function TMainForm.NewHttpClient(const AResponseTimeout: Integer): THTTPClient;
begin
  Result := THTTPClient.Create;
  Result.ConnectionTimeout := 2000;
  Result.ResponseTimeout := AResponseTimeout;
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

function TMainForm.GetJsonWith(AClient: THTTPClient; const APath: string): TJSONObject;
var
  LResponse: IHTTPResponse;
  LValue: TJSONValue;
begin
  LResponse := AClient.Get(FBridgeUrl + APath);
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

function TMainForm.PostJsonWith(AClient: THTTPClient; const APath,
  AJson: string): TJSONObject;
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
    LResponse := AClient.Post(
      FBridgeUrl + APath,
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

function TMainForm.ReadStateListen(const AStatePath: string): string;
var
  LValue: TJSONValue;
begin
  Result := '';
  if (AStatePath = '') or not FileExists(AStatePath) then
    Exit;
  LValue := TJSONObject.ParseJSONValue(TFile.ReadAllText(AStatePath));
  if LValue is TJSONObject then
    Result := JsonText(TJSONObject(LValue), 'listen');
  LValue.Free;
  if Result <> '' then
    Result := 'http://' + Result;
end;

function TMainForm.ReadServiceStateFilePath: string;
var
  LKey: HKEY;
  LValueType: DWORD;
  LSize: DWORD;
  LBuffer: TBytes;
  LText: string;
  LPart: string;
begin
  Result := '';
  if RegOpenKeyEx(
    HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Services\ClaudeCodeBridge',
    0,
    KEY_READ,
    LKey
  ) <> ERROR_SUCCESS then
    Exit;
  try
    LSize := 0;
    if RegQueryValueEx(LKey, 'Environment', nil, @LValueType, nil, @LSize) <>
      ERROR_SUCCESS then
      Exit;
    if (LValueType <> REG_MULTI_SZ) or (LSize = 0) then
      Exit;
    SetLength(LBuffer, LSize);
    if RegQueryValueEx(LKey, 'Environment', nil, @LValueType, @LBuffer[0],
      @LSize) <> ERROR_SUCCESS then
      Exit;
    LText := TEncoding.Unicode.GetString(LBuffer);
    for LPart in LText.Split([#0]) do
      if LPart.StartsWith('GEMINI_BRIDGE_STATE_FILE=') then
        Exit(LPart.Substring(Length('GEMINI_BRIDGE_STATE_FILE=')));
  finally
    RegCloseKey(LKey);
  end;
end;

function TMainForm.ResolveBridgeUrl: string;
var
  LStatePath: string;
  LListenUrl: string;
begin
  Result := DEFAULT_BRIDGE_URL;
  try
    LStatePath := GetEnvironmentVariable('GEMINI_BRIDGE_STATE_FILE');
    if LStatePath = '' then
      LStatePath := FBridgeRoot + '\bridge-state.json';
    LListenUrl := ReadStateListen(LStatePath);
    if (LListenUrl = '') and
       (not SameText(LStatePath, FBridgeRoot + '\bridge-state.json')) then
      Exit;
    if LListenUrl = '' then
      LListenUrl := ReadStateListen(ReadServiceStateFilePath);
    if LListenUrl <> '' then
      Result := LListenUrl;
  except
    Result := DEFAULT_BRIDGE_URL;
  end;
end;

procedure TMainForm.UpdateProxyFromStatus(const AServerProxy: string);
begin
  if SameText(Trim(FProxyEdit.Text), Trim(AServerProxy)) then
  begin
    FProxyDirty := False;
    Exit;
  end;
  if FProxyDirty or FProxyEdit.Focused then
    Exit;
  FUpdatingProxy := True;
  try
    FProxyEdit.Text := AServerProxy;
  finally
    FUpdatingProxy := False;
  end;
end;

procedure TMainForm.ProxyEditChange(Sender: TObject);
begin
  if not FUpdatingProxy then
    FProxyDirty := True;
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

procedure TMainForm.SetBusy(const ABusy: Boolean);
begin
  FBusy := ABusy;
  if ABusy then
  begin
    FRefreshButton.Enabled := False;
    FProfileList.Enabled := False;
    FSwitchButton.Enabled := False;
    FStartButton.Enabled := False;
    FStopButton.Enabled := False;
    FTestProxyButton.Enabled := False;
    FApplyProxyButton.Enabled := False;
    FDetectProxyButton.Enabled := False;
    Screen.Cursor := crHourGlass;
  end
  else
  begin
    Screen.Cursor := crDefault;
    FRefreshButton.Enabled := True;
    FProfileList.Enabled := True;
    FDetectProxyButton.Enabled := True;
    RefreshStatusAsync(True);
  end;
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
  if FBusy then
  begin
    FRefreshButton.Enabled := False;
    FSwitchButton.Enabled := False;
    FStartButton.Enabled := False;
    FStopButton.Enabled := False;
    FTestProxyButton.Enabled := False;
    FApplyProxyButton.Enabled := False;
  end;
  FStatusBar.Panels[0].Text := ADetail;
end;

procedure TMainForm.RefreshStatusAsync(const AQuiet: Boolean);
begin
  if FClosing or FStatusInFlight then
    Exit;
  FStatusInFlight := True;
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LClient: THTTPClient;
      LJson: TJSONObject;
      LProfile: TJSONObject;
      LSnapshot: TStatusSnapshot;
    begin
      LSnapshot := Default(TStatusSnapshot);
      try
        LClient := NewHttpClient(STATUS_RESPONSE_TIMEOUT_MS);
        try
          LJson := GetJsonWith(LClient, '/admin/status');
          try
            LSnapshot.Online := True;
            if LJson.Values['active_profile'] is TJSONObject then
            begin
              LProfile := TJSONObject(LJson.Values['active_profile']);
              LSnapshot.HasProfile := True;
              LSnapshot.Model := JsonText(LProfile, 'model');
              LSnapshot.FileName := JsonText(LProfile, 'file');
            end;
            LSnapshot.Proxy := JsonText(LJson, 'gemini_proxy');
            LSnapshot.Stamp := JsonText(LJson, 'config_stamp');
            if LSnapshot.Stamp = '' then
              LSnapshot.Stamp := JsonText(LJson, 'settings_stamp');
            LSnapshot.SettingsDir := JsonText(LJson, 'providers_dir');
            if LSnapshot.SettingsDir = '' then
              LSnapshot.SettingsDir := JsonText(LJson, 'settings_dir');
          finally
            LJson.Free;
          end;
        finally
          LClient.Free;
        end;
      except
        on E: Exception do
        begin
          LSnapshot.Online := False;
          LSnapshot.Detail := E.Message;
        end;
      end;
      QueueToMain(
        procedure
        begin
          FStatusInFlight := False;
          ApplyStatusSnapshot(LSnapshot, AQuiet);
        end);
    end).Start;
end;

procedure TMainForm.ApplyStatusSnapshot(
  const ASnapshot: TStatusSnapshot;
  const AQuiet: Boolean);
var
  LStampChanged: Boolean;
begin
  if ASnapshot.Online then
  begin
    UpdateProxyFromStatus(ASnapshot.Proxy);
    if ASnapshot.HasProfile then
    begin
      FActiveFile := ASnapshot.FileName;
      FCurrentModelLabel.Caption := '当前模型：' + ASnapshot.Model;
      SetBridgeState(True, '当前配置：' + ASnapshot.FileName);
    end
    else
    begin
      FActiveFile := '';
      FCurrentModelLabel.Caption := '当前模型：暂无可用配置';
      SetBridgeState(
        True,
        '桥接器运行中，请在 bridge-providers 目录添加 JSON 配置'
      );
      FSwitchButton.Enabled := False;
    end;
    if ASnapshot.SettingsDir <> '' then
      FStatusBar.Panels[1].Text := '配置目录：' + ASnapshot.SettingsDir;
    LStampChanged :=
      (ASnapshot.Stamp <> '') and (ASnapshot.Stamp <> FSettingsStamp);
    if LStampChanged then
    begin
      FSettingsStamp := ASnapshot.Stamp;
      StartProfilesRefresh(FProfilesLoaded);
    end;
  end
  else
  begin
    SetBridgeState(False, ASnapshot.Detail);
    if not AQuiet then
      AppendLog('状态检查失败：' + ASnapshot.Detail);
  end;
end;

procedure TMainForm.StartProfilesRefresh(const AQuiet: Boolean);
var
  LEditProxy: string;
begin
  if FClosing or FProfilesBusy then
    Exit;
  FProfilesBusy := True;
  LEditProxy := Trim(FProxyEdit.Text);
  if not AQuiet then
    SetBusy(True);
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LClient: THTTPClient;
      LReload: TJSONObject;
      LJson: TJSONObject;
      LArray: TJSONArray;
      LProfile: TJSONObject;
      LRows: TArray<TProfileRow>;
      LRow: TProfileRow;
      LStamp: string;
      LTransport: string;
      LName: string;
      LError: string;
      I: Integer;
    begin
      LError := '';
      try
        LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
        try
          LReload := PostJsonWith(LClient, '/admin/reload-profiles', '{}');
          LReload.Free;
          LJson := GetJsonWith(LClient, '/admin/profiles');
          try
            LArray := LJson.Values['profiles'] as TJSONArray;
            SetLength(LRows, LArray.Count);
            for I := 0 to LArray.Count - 1 do
            begin
              LProfile := LArray.Items[I] as TJSONObject;
              LRow := Default(TProfileRow);
              LRow.Model := JsonText(LProfile, 'model');
              LName := JsonText(LProfile, 'name');
              if (LName <> '') and not SameText(LName, LRow.Model) then
                LRow.Model := LName + ' (' + LRow.Model + ')';
              LRow.FileName := JsonText(LProfile, 'file');
              LRow.BaseUrl := JsonText(LProfile, 'base_url');
              LRow.Proxy := JsonText(LProfile, 'proxy');
              LRow.Active := JsonBool(LProfile, 'active');
              LTransport := JsonText(LProfile, 'transport');
              if SameText(LTransport, 'gemini') or
                 JsonBool(LProfile, 'local_gemini') then
              begin
                LRow.Route := 'Gemini 转换';
                LRow.Proxy := LEditProxy;
              end
              else if SameText(LTransport, 'openai-chat') then
                LRow.Route := 'OpenAI 转换'
              else
                LRow.Route := 'Anthropic 直通';
              if LRow.Proxy = '' then
                LRow.Proxy := '(直连)';
              LRows[I] := LRow;
            end;
            LStamp := JsonText(LJson, 'config_stamp');
            if LStamp = '' then
              LStamp := JsonText(LJson, 'settings_stamp');
          finally
            LJson.Free;
          end;
        finally
          LClient.Free;
        end;
      except
        on E: Exception do
          LError := E.Message;
      end;
      QueueToMain(
        procedure
        begin
          FProfilesBusy := False;
          if not AQuiet then
            SetBusy(False);
          if LError <> '' then
          begin
            AppendLog('刷新失败：' + LError);
            if not AQuiet then
              MessageDlg(LError, mtError, [mbOK], 0);
          end
          else
          begin
            FillProfileList(LRows);
            FProfilesLoaded := True;
            FSettingsStamp := LStamp;
            AppendLog(Format('已加载 %d 个模型配置。', [Length(LRows)]));
          end;
        end);
    end).Start;
end;

procedure TMainForm.FillProfileList(const ARows: TArray<TProfileRow>);
var
  I: Integer;
  LItem: TListItem;
  LRow: TProfileRow;
begin
  FProfileList.Items.BeginUpdate;
  try
    FProfileList.Items.Clear;
    FProfileFiles.Clear;
    for I := 0 to High(ARows) do
    begin
      LRow := ARows[I];
      LItem := FProfileList.Items.Add;
      if LRow.Active then
      begin
        LItem.Caption := '✓  ' + LRow.Model;
        LItem.Selected := True;
        LItem.Focused := True;
        FActiveFile := LRow.FileName;
      end
      else
        LItem.Caption := LRow.Model;
      LItem.SubItems.Add(LRow.BaseUrl);
      LItem.SubItems.Add(LRow.Proxy);
      LItem.SubItems.Add(LRow.Route);
      LItem.SubItems.Add(LRow.FileName);
      FProfileFiles.Add(LRow.FileName);
    end;
  finally
    FProfileList.Items.EndUpdate;
  end;
  AdjustProfileColumns;
end;

procedure TMainForm.ListSelectItem(Sender: TObject; Item: TListItem;
  Selected: Boolean);
begin
  if Selected and (Item.SubItems.Count >= 4) then
    FStatusBar.Panels[2].Text := '选中：' + Item.SubItems[3];
end;

procedure TMainForm.SwitchProfileAsync(const AFileName: string);
begin
  if FClosing or FBusy then
    Exit;
  SetBusy(True);
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LClient: THTTPClient;
      LRequest: TJSONObject;
      LResult: TJSONObject;
      LError: string;
    begin
      LError := '';
      try
        LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
        try
          LRequest := TJSONObject.Create;
          try
            LRequest.AddPair('file', AFileName);
            LResult := PostJsonWith(
              LClient,
              '/admin/active-profile',
              LRequest.ToJSON
            );
          finally
            LRequest.Free;
          end;
          LResult.Free;
        finally
          LClient.Free;
        end;
      except
        on E: Exception do
          LError := E.Message;
      end;
      QueueToMain(
        procedure
        begin
          SetBusy(False);
          if LError <> '' then
          begin
            AppendLog('切换失败：' + LError);
            MessageDlg(LError, mtError, [mbOK], 0);
          end
          else
          begin
            AppendLog('模型已切换：' + AFileName);
            StartProfilesRefresh(True);
          end;
        end);
    end).Start;
end;

procedure TMainForm.RefreshClick(Sender: TObject);
begin
  StartProfilesRefresh(False);
end;

procedure TMainForm.SwitchClick(Sender: TObject);
begin
  if not Assigned(FProfileList.Selected) then
  begin
    MessageDlg('请先选择一个模型配置。', mtInformation, [mbOK], 0);
    Exit;
  end;
  if (FProfileList.Selected.Index < 0) or
     (FProfileList.Selected.Index >= FProfileFiles.Count) then
    Exit;
  SwitchProfileAsync(FProfileFiles[FProfileList.Selected.Index]);
end;

procedure TMainForm.ProfileDblClick(Sender: TObject);
var
  LPos: TPoint;
  LItem: TListItem;
begin
  LPos := FProfileList.ScreenToClient(Mouse.CursorPos);
  LItem := FProfileList.GetItemAt(LPos.X, LPos.Y);
  if (LItem <> nil) and (LItem.Index >= 0) and
     (LItem.Index < FProfileFiles.Count) then
    SwitchProfileAsync(FProfileFiles[LItem.Index]);
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

procedure TMainForm.StartStopAsync(const AStart: Boolean);
begin
  if FClosing then
    Exit;
  SetBusy(True);
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LExitCode: Cardinal;
      LError: string;
      LScript: string;
      LLogOk: string;
      LLogFail: string;
    begin
      LError := '';
      if AStart then
      begin
        LScript := FBridgeRoot + '\scripts\start-bridge.ps1';
        LLogOk := '桥接器已启动。';
        LLogFail := '启动失败：';
      end
      else
      begin
        LScript := FBridgeRoot + '\scripts\stop-bridge.ps1';
        LLogOk := '桥接器已停止。';
        LLogFail := '停止失败：';
      end;
      try
        LExitCode := RunPowerShellScript(LScript);
        if LExitCode <> 0 then
          raise Exception.CreateFmt('脚本返回错误码 %d', [LExitCode]);
        if AStart then
          Sleep(500);
      except
        on E: Exception do
          LError := E.Message;
      end;
      QueueToMain(
        procedure
        begin
          SetBusy(False);
          if LError <> '' then
          begin
            AppendLog(LLogFail + LError);
            MessageDlg(LError, mtError, [mbOK], 0);
          end
          else
            AppendLog(LLogOk);
        end);
    end).Start;
end;

procedure TMainForm.StartClick(Sender: TObject);
begin
  StartStopAsync(True);
end;

procedure TMainForm.StopClick(Sender: TObject);
begin
  StartStopAsync(False);
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
  LBody: string;
begin
  if FClosing or FBusy then
    Exit;
  LBody := ProxyRequestJson;
  FTestProxyButton.Enabled := False;
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LClient: THTTPClient;
      LResult: TJSONObject;
      LModel: string;
      LError: string;
    begin
      LError := '';
      try
        LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
        try
          LResult := PostJsonWith(LClient, '/admin/gemini-proxy/test', LBody);
          LModel := JsonText(LResult, 'model');
          LResult.Free;
        finally
          LClient.Free;
        end;
      except
        on E: Exception do
          LError := E.Message;
      end;
      QueueToMain(
        procedure
        begin
          FTestProxyButton.Enabled := (not FBusy) and (FStatusLabel.Tag = 1);
          if LError <> '' then
          begin
            AppendLog('Gemini 连接测试失败：' + LError);
            MessageDlg(LError, mtError, [mbOK], 0);
          end
          else
          begin
            AppendLog('Gemini 连接测试成功，模型：' + LModel);
            MessageDlg('Gemini 连接测试成功。', mtInformation, [mbOK], 0);
          end;
        end);
    end).Start;
end;

procedure TMainForm.ApplyProxyClick(Sender: TObject);
var
  LBody: string;
begin
  if FClosing or FBusy then
    Exit;
  LBody := ProxyRequestJson;
  SetBusy(True);
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LClient: THTTPClient;
      LResult: TJSONObject;
      LProxy: string;
      LError: string;
    begin
      LError := '';
      try
        LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
        try
          LResult := PostJsonWith(LClient, '/admin/gemini-proxy', LBody);
          LProxy := JsonText(LResult, 'gemini_proxy');
          LResult.Free;
        finally
          LClient.Free;
        end;
      except
        on E: Exception do
          LError := E.Message;
      end;
      QueueToMain(
        procedure
        begin
          SetBusy(False);
          if LError <> '' then
          begin
            AppendLog('保存 Gemini 代理失败：' + LError);
            MessageDlg(LError, mtError, [mbOK], 0);
          end
          else
          begin
            FProxyDirty := False;
            if LProxy = '' then
              AppendLog('Gemini 已切换为直连。')
            else
              AppendLog('Gemini 代理已保存并立即生效：' + LProxy);
            StartProfilesRefresh(True);
          end;
        end);
    end).Start;
end;

procedure TMainForm.PollTimer(Sender: TObject);
begin
  RefreshStatusAsync(True);
end;

end.
