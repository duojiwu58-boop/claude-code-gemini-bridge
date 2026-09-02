unit BridgeManagerMain;

interface

uses
  Winapi.Windows,
  Winapi.Messages,
  Winapi.CommCtrl,
  Winapi.UxTheme,
  Winapi.ShellAPI,
  System.SysUtils,
  System.Classes,
  System.Generics.Collections,
  System.JSON,
  System.UITypes,
  System.Math,
  System.IOUtils,
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
    Transport: string;
    ThinkingLevel: string;
    Stamp: string;
    SettingsDir: string;
    Detail: string;
    ComputerHostConnected: Boolean;
    ComputerSessionActive: Boolean;
    ComputerStatus: string;
    ComputerUrl: string;
    ComputerIntent: string;
    ComputerSelectedWindow: string;
    ComputerSelectedHwnd: string;
    ComputerApprovalPending: Boolean;
    ComputerApprovalSession: string;
    ComputerApprovalBatch: string;
    ComputerApprovalHash: string;
    ComputerApprovalText: string;
    ComputerScreenshotData: string;
    ComputerScreenshotHash: string;
    ComputerLogs: string;
  end;

  TMainForm = class(TForm)
  private
    FBridgeUrl: string;
    FLocalAuthToken: string;
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
    FThinkingGroup: TRadioGroup;
    FContentPanel: TPanel;
    FListPanel: TPanel;
    FListTitleLabel: TLabel;
    FListHintLabel: TLabel;
    FProfileList: TListView;
    FLogPanel: TPanel;
    FLogTitleLabel: TLabel;
    FLogMemo: TMemo;
    FComputerPanel: TPanel;
    FComputerTitleLabel: TLabel;
    FComputerStatusLabel: TLabel;
    FComputerUrlLabel: TLabel;
    FComputerUrlEdit: TEdit;
    FComputerEnvironment: TComboBox;
    FComputerEnableButton: TButton;
    FComputerStopButton: TButton;
    FComputerWindowButton: TButton;
    FComputerIntentLabel: TLabel;
    FComputerApprovalLabel: TLabel;
    FComputerAllowButton: TButton;
    FComputerRejectButton: TButton;
    FComputerPolicyLabel: TLabel;
    FComputerImage: TImage;
    FComputerLogMemo: TMemo;
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
    FSettingsStamp: string;
    FThinkingLevel: string;
    FUpdatingThinkingUi: Boolean;
    FWorkerCount: Integer;
    FComputerApprovalSession: string;
    FComputerApprovalBatch: string;
    FComputerApprovalHash: string;
    procedure FormShown(Sender: TObject);
    procedure FormResize(Sender: TObject);
    procedure RefreshClick(Sender: TObject);
    procedure SwitchClick(Sender: TObject);
    procedure StartClick(Sender: TObject);
    procedure StopClick(Sender: TObject);
    procedure ThinkingLevelClick(Sender: TObject);
    procedure PollTimer(Sender: TObject);
    procedure ComputerEnableClick(Sender: TObject);
    procedure ComputerEnvironmentChange(Sender: TObject);
    procedure ComputerStopClick(Sender: TObject);
    procedure ComputerWindowClick(Sender: TObject);
    procedure ComputerAllowClick(Sender: TObject);
    procedure ComputerRejectClick(Sender: TObject);
    procedure ProfileDblClick(Sender: TObject);
    procedure ListSelectItem(Sender: TObject; Item: TListItem;
      Selected: Boolean);
    procedure BuildUi;
    procedure ApplyWindowsAppearance;
    procedure AppendLog(const AText: string);
    procedure QueueToMain(const AProc: TProc);
    procedure SetBusy(const ABusy: Boolean);
    procedure RefreshStatusAsync(const AQuiet: Boolean);
    procedure StartProfilesRefresh(const AQuiet: Boolean);
    procedure SwitchProfileAsync(const AFileName: string);
    procedure SetThinkingLevelAsync(const ALevel: string);
    procedure StartStopAsync(const AStart: Boolean);
    procedure ApplyStatusSnapshot(const ASnapshot: TStatusSnapshot;
      const AQuiet: Boolean);
    procedure ApplyThinkingControls(const ASnapshot: TStatusSnapshot);
    procedure ApplyComputerSnapshot(const ASnapshot: TStatusSnapshot);
    procedure DecideComputerApprovalAsync(const AApprove: Boolean);
    procedure StopComputerAsync;
    procedure FillProfileList(const ARows: TArray<TProfileRow>);
    procedure AdjustProfileColumns;
    procedure SetBridgeState(const AOnline: Boolean; const ADetail: string);
    function NewHttpClient(const AResponseTimeout: Integer): THTTPClient;
    function GetJsonWith(AClient: THTTPClient; const APath: string): TJSONObject;
    function PostJsonWith(AClient: THTTPClient; const APath,
      AJson: string): TJSONObject;
    function JsonText(AObject: TJSONObject; const AName: string): string;
    function JsonBool(AObject: TJSONObject; const AName: string): Boolean;
    function FindBridgeRoot: string;
    function ResolveBridgeUrl: string;
    function ReadLocalAuthToken: string;
    function LocalAuthTokenPath: string;
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
  SCRIPT_PROCESS_TIMEOUT_MS = 5 * 60 * 1000;
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
    Height := 940;
    Position := poScreenCenter;
    Constraints.MinWidth := 900;
    Constraints.MinHeight := 820;
    Color := $00F5F5F5;
    Font.Name := 'Segoe UI';
    Font.Size := 10;
    DoubleBuffered := True;
    Icon.Assign(Application.Icon);

    LStage := '创建配置列表';
    FProfileFiles := TStringList.Create;
    FBridgeRoot := FindBridgeRoot;
    FBridgeUrl := ResolveBridgeUrl;
    FLocalAuthToken := ReadLocalAuthToken;

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
  FHintLabel.Caption := '统一管理模型路由、Provider 配置和桥接服务';
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

  FThinkingGroup := TRadioGroup.Create(Self);
  FThinkingGroup.Parent := FTopPanel;
  FThinkingGroup.SetBounds(596, 78, 400, 70);
  FThinkingGroup.Caption := 'Gemini Thinking（下一请求生效）';
  FThinkingGroup.Columns := 3;
  FThinkingGroup.Items.Add('低');
  FThinkingGroup.Items.Add('中');
  FThinkingGroup.Items.Add('高');
  FThinkingGroup.ItemIndex := -1;
  FThinkingGroup.Enabled := False;
  FThinkingGroup.OnClick := ThinkingLevelClick;

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

  FComputerPanel := TPanel.Create(Self);
  FComputerPanel.Parent := FContentPanel;
  FComputerPanel.Align := alBottom;
  FComputerPanel.AlignWithMargins := True;
  FComputerPanel.Margins.SetBounds(0, 12, 0, 0);
  FComputerPanel.Height := 62;
  FComputerPanel.BevelOuter := bvNone;
  FComputerPanel.Color := clWhite;
  FComputerPanel.ParentBackground := False;

  FComputerTitleLabel := TLabel.Create(Self);
  FComputerTitleLabel.Parent := FComputerPanel;
  FComputerTitleLabel.SetBounds(16, 12, 220, 24);
  FComputerTitleLabel.Caption := 'Computer Use MCP';
  FComputerTitleLabel.Font.Style := [fsBold];
  FComputerTitleLabel.Font.Size := 11;

  FComputerStatusLabel := TLabel.Create(Self);
  FComputerStatusLabel.Parent := FComputerPanel;
  FComputerStatusLabel.SetBounds(180, 15, 760, 20);
  FComputerStatusLabel.Caption := '由 Claude Code 自动拉起；选窗和安全确认由 Host 按需弹出';

  FComputerEnvironment := TComboBox.Create(Self);
  FComputerEnvironment.Parent := FComputerPanel;
  FComputerEnvironment.SetBounds(16, 44, 105, 28);
  FComputerEnvironment.Style := csDropDownList;
  FComputerEnvironment.Items.Add('Browser');
  FComputerEnvironment.Items.Add('Desktop');
  FComputerEnvironment.ItemIndex := 0;
  FComputerEnvironment.OnChange := ComputerEnvironmentChange;
  FComputerEnvironment.Visible := False;

  FComputerUrlLabel := TLabel.Create(Self);
  FComputerUrlLabel.Parent := FComputerPanel;
  FComputerUrlLabel.SetBounds(132, 48, 62, 20);
  FComputerUrlLabel.Caption := '本地 URL';
  FComputerUrlLabel.Visible := False;

  FComputerUrlEdit := TEdit.Create(Self);
  FComputerUrlEdit.Parent := FComputerPanel;
  FComputerUrlEdit.SetBounds(195, 44, 284, 28);
  FComputerUrlEdit.Text := 'http://127.0.0.1:3000/';
  FComputerUrlEdit.Visible := False;

  FComputerEnableButton := TButton.Create(Self);
  FComputerEnableButton.Parent := FComputerPanel;
  FComputerEnableButton.SetBounds(489, 42, 92, 32);
  FComputerEnableButton.Caption := '启用 Host';
  FComputerEnableButton.OnClick := ComputerEnableClick;
  FComputerEnableButton.Visible := False;

  FComputerStopButton := TButton.Create(Self);
  FComputerStopButton.Parent := FComputerPanel;
  FComputerStopButton.SetBounds(589, 42, 92, 32);
  FComputerStopButton.Caption := '立即停止';
  FComputerStopButton.OnClick := ComputerStopClick;
  FComputerStopButton.Visible := False;

  FComputerWindowButton := TButton.Create(Self);
  FComputerWindowButton.Parent := FComputerPanel;
  FComputerWindowButton.SetBounds(589, 80, 92, 28);
  FComputerWindowButton.Caption := '选择窗口';
  FComputerWindowButton.Enabled := False;
  FComputerWindowButton.OnClick := ComputerWindowClick;
  FComputerWindowButton.Visible := False;

  FComputerIntentLabel := TLabel.Create(Self);
  FComputerIntentLabel.Parent := FComputerPanel;
  FComputerIntentLabel.SetBounds(16, 82, 560, 20);
  FComputerIntentLabel.Caption := 'Gemini intent：-';
  FComputerIntentLabel.Visible := False;

  FComputerApprovalLabel := TLabel.Create(Self);
  FComputerApprovalLabel.Parent := FComputerPanel;
  FComputerApprovalLabel.SetBounds(16, 108, 460, 38);
  FComputerApprovalLabel.AutoSize := False;
  FComputerApprovalLabel.WordWrap := True;
  FComputerApprovalLabel.Caption := '待确认动作：无';
  FComputerApprovalLabel.Visible := False;

  FComputerAllowButton := TButton.Create(Self);
  FComputerAllowButton.Parent := FComputerPanel;
  FComputerAllowButton.SetBounds(489, 112, 92, 30);
  FComputerAllowButton.Caption := '允许一次';
  FComputerAllowButton.Enabled := False;
  FComputerAllowButton.OnClick := ComputerAllowClick;
  FComputerAllowButton.Visible := False;

  FComputerRejectButton := TButton.Create(Self);
  FComputerRejectButton.Parent := FComputerPanel;
  FComputerRejectButton.SetBounds(589, 112, 92, 30);
  FComputerRejectButton.Caption := '拒绝';
  FComputerRejectButton.Enabled := False;
  FComputerRejectButton.OnClick := ComputerRejectClick;
  FComputerRejectButton.Visible := False;

  FComputerPolicyLabel := TLabel.Create(Self);
  FComputerPolicyLabel.Parent := FComputerPanel;
  FComputerPolicyLabel.SetBounds(16, 150, 665, 20);
  FComputerPolicyLabel.Caption := '限制：50 步 / 15 分钟 ｜ Desktop 仅限所选窗口及同进程对话框';
  FComputerPolicyLabel.Font.Color := $00606060;
  FComputerPolicyLabel.Visible := False;

  FComputerLogMemo := TMemo.Create(Self);
  FComputerLogMemo.Parent := FComputerPanel;
  FComputerLogMemo.SetBounds(16, 174, 665, 56);
  FComputerLogMemo.ReadOnly := True;
  FComputerLogMemo.ScrollBars := ssVertical;
  FComputerLogMemo.Font.Name := 'Cascadia Mono';
  FComputerLogMemo.Font.Size := 8;
  FComputerLogMemo.Visible := False;

  FComputerImage := TImage.Create(Self);
  FComputerImage.Parent := FComputerPanel;
  FComputerImage.SetBounds(697, 42, 280, 188);
  FComputerImage.Stretch := True;
  FComputerImage.Proportional := True;
  FComputerImage.Center := True;
  FComputerImage.Anchors := [akTop, akRight, akBottom];
  FComputerImage.Visible := False;

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
    FThinkingGroup.Left :=
      FTopPanel.ClientWidth - EDGE_MARGIN - FThinkingGroup.Width;
  end;

  if Assigned(FStatusBar) then
  begin
    FStatusBar.Panels[0].Width := FStatusBar.ClientWidth * 2 div 5;
    FStatusBar.Panels[1].Width := FStatusBar.ClientWidth * 3 div 10;
  end;

  if Assigned(FComputerImage) and Assigned(FComputerPanel) then
  begin
    FComputerImage.Left := FComputerPanel.ClientWidth - EDGE_MARGIN -
      FComputerImage.Width;
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
  LHeaders: TNetHeaders;
  LResponse: IHTTPResponse;
  LValue: TJSONValue;
begin
  SetLength(LHeaders, 1);
  LHeaders[0].Name := 'Authorization';
  LHeaders[0].Value := 'Bearer ' + FLocalAuthToken;
  LResponse := AClient.Get(FBridgeUrl + APath, nil, LHeaders);
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
  SetLength(LHeaders, 2);
  LHeaders[0].Name := 'Content-Type';
  LHeaders[0].Value := 'application/json';
  LHeaders[1].Name := 'Authorization';
  LHeaders[1].Value := 'Bearer ' + FLocalAuthToken;
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

function TMainForm.ReadLocalAuthToken: string;
begin
  if not FileExists(LocalAuthTokenPath) then
    raise Exception.Create('找不到桥接器本地认证令牌，请重新运行安装程序');
  Result := Trim(TFile.ReadAllText(LocalAuthTokenPath, TEncoding.UTF8));
  if Length(Result) < 32 then
    raise Exception.Create('桥接器本地认证令牌无效，请重新运行安装程序');
end;

function TMainForm.LocalAuthTokenPath: string;
var
  LProgramData: string;
begin
  LProgramData := GetEnvironmentVariable('ProgramData');
  if LProgramData = '' then
    raise Exception.Create('无法定位 ProgramData，不能读取桥接器本地认证令牌');
  Result := TPath.Combine(
    TPath.Combine(LProgramData, 'ClaudeCodeBridge'),
    'local-auth-token'
  );
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
    FThinkingGroup.Enabled := False;
    Screen.Cursor := crHourGlass;
  end
  else
  begin
    Screen.Cursor := crDefault;
    FRefreshButton.Enabled := True;
    FProfileList.Enabled := True;
    FThinkingGroup.Enabled :=
      (FStatusLabel.Tag = 1) and (FThinkingLevel <> '');
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
  end
  else
  begin
    FStatusLabel.Caption := '桥接器：未运行';
    FStatusLabel.Font.Color := $001C2BC4;
    FStatusDotLabel.Font.Color := $001C2BC4;
    FStatusPanel.Color := $00EDEFFB;
    FStatusLabel.Tag := 0;
    FCurrentModelLabel.Caption := '当前模型：-';
    FThinkingGroup.Enabled := False;
    FSwitchButton.Enabled := False;
    FStopButton.Enabled := False;
    FStartButton.Enabled := True;
  end;
  if FBusy then
  begin
    FRefreshButton.Enabled := False;
    FSwitchButton.Enabled := False;
    FStartButton.Enabled := False;
    FStopButton.Enabled := False;
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
              LSnapshot.Transport := JsonText(LProfile, 'transport');
            end;
            LSnapshot.ThinkingLevel := JsonText(
              LJson,
              'gemini_thinking_level'
            );
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
    ApplyThinkingControls(ASnapshot);
    ApplyComputerSnapshot(ASnapshot);
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
    ApplyThinkingControls(ASnapshot);
    ApplyComputerSnapshot(ASnapshot);
    if not AQuiet then
      AppendLog('状态检查失败：' + ASnapshot.Detail);
  end;
end;

procedure TMainForm.ApplyThinkingControls(const ASnapshot: TStatusSnapshot);
var
  LSupported: Boolean;
begin
  LSupported := ASnapshot.Online and ASnapshot.HasProfile and
    SameText(ASnapshot.Transport, 'gemini-interactions') and
    (ASnapshot.ThinkingLevel <> '');
  FUpdatingThinkingUi := True;
  try
    if LSupported then
    begin
      FThinkingGroup.Caption := 'Gemini Thinking（下一请求生效）';
      FThinkingLevel := LowerCase(ASnapshot.ThinkingLevel);
      if SameText(FThinkingLevel, 'low') then
        FThinkingGroup.ItemIndex := 0
      else if SameText(FThinkingLevel, 'medium') then
        FThinkingGroup.ItemIndex := 1
      else if SameText(FThinkingLevel, 'high') then
        FThinkingGroup.ItemIndex := 2
      else
        FThinkingGroup.ItemIndex := -1;
      FThinkingGroup.Enabled := not FBusy;
    end
    else
    begin
      FThinkingLevel := '';
      FThinkingGroup.ItemIndex := -1;
      FThinkingGroup.Enabled := False;
      FThinkingGroup.Caption := 'Gemini Thinking（3.7+ Flash）';
    end;
  finally
    FUpdatingThinkingUi := False;
  end;
end;

procedure TMainForm.ApplyComputerSnapshot(const ASnapshot: TStatusSnapshot);
begin
  FComputerStatusLabel.Caption :=
    '由 Claude Code 自动拉起；选窗和安全确认由 Host 按需弹出';
end;

procedure TMainForm.ComputerEnableClick(Sender: TObject);
begin
  MessageDlg('Computer Host 已改为 Claude Code 托管的本地 stdio MCP，' +
    '无需从模型中心启动。', mtInformation, [mbOK], 0);
end;

procedure TMainForm.ComputerEnvironmentChange(Sender: TObject);
begin
  FComputerUrlLabel.Enabled := FComputerEnvironment.ItemIndex = 0;
  FComputerUrlEdit.Enabled := FComputerEnvironment.ItemIndex = 0;
  FComputerWindowButton.Enabled := (FComputerEnvironment.ItemIndex = 1) and
    (Pos('Host：已连接', FComputerStatusLabel.Caption) > 0) and
    (Pos('会话：运行中', FComputerStatusLabel.Caption) = 0);
  if FComputerEnvironment.ItemIndex = 1 then
    FComputerPolicyLabel.Caption := 'Desktop：WGC 截图 + SendInput，仅限所选 HWND、子窗口和同进程对话框'
  else
    FComputerPolicyLabel.Caption := 'Browser：最大 50 步 / 15 分钟，仅允许 localhost、127.0.0.1、::1';
end;

procedure TMainForm.ComputerStopClick(Sender: TObject);
begin
  StopComputerAsync;
end;

procedure TMainForm.ComputerWindowClick(Sender: TObject);
var
  LClient: THTTPClient;
  LResponse: TJSONObject;
  LWindows: TJSONArray;
  LWindow: TJSONObject;
  LHandles: TStringList;
  LDialog: TForm;
  LList: TListBox;
  LOk: TButton;
  LCancel: TButton;
  LIndex: Integer;
  LBlocked: Integer;
  LRequest: TJSONObject;
  LSelectedResponse: TJSONObject;
  LTitle: string;
  LPath: string;
begin
  if FComputerEnvironment.ItemIndex <> 1 then
  begin
    MessageDlg('请先把环境切换为 Desktop。', mtInformation, [mbOK], 0);
    Exit;
  end;
  LHandles := TStringList.Create;
  try
    try
      LBlocked := 0;
      LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
      try
        LResponse := GetJsonWith(LClient, '/admin/computer/windows');
        try
          if not (LResponse.Values['windows'] is TJSONArray) then
            raise Exception.Create('Computer Host 未返回窗口列表。');
          LWindows := TJSONArray(LResponse.Values['windows']);
          LDialog := TForm.Create(Self);
          try
          LDialog.Caption := '选择 Desktop Computer Use 目标窗口';
          LDialog.Position := poOwnerFormCenter;
          LDialog.BorderStyle := bsDialog;
          LDialog.ClientWidth := 720;
          LDialog.ClientHeight := 390;
          LList := TListBox.Create(LDialog);
          LList.Parent := LDialog;
          LList.SetBounds(16, 16, 688, 318);
          LList.Anchors := [akLeft, akTop, akRight, akBottom];
          LList.ItemHeight := 22;
          for LIndex := 0 to LWindows.Count - 1 do
            if LWindows.Items[LIndex] is TJSONObject then
            begin
              LWindow := TJSONObject(LWindows.Items[LIndex]);
              if JsonBool(LWindow, 'eligible') then
              begin
                LTitle := JsonText(LWindow, 'title');
                LPath := ExtractFileName(JsonText(LWindow, 'process_path'));
                LList.Items.Add(Format('%s  —  %s  [HWND %s]',
                  [LTitle, LPath, JsonText(LWindow, 'hwnd')]));
                LHandles.Add(JsonText(LWindow, 'hwnd'));
              end
              else
                Inc(LBlocked);
            end;
          if LList.Items.Count > 0 then
            LList.ItemIndex := 0;
          LOk := TButton.Create(LDialog);
          LOk.Parent := LDialog;
          LOk.SetBounds(512, 346, 92, 30);
          LOk.Caption := '选择';
          LOk.ModalResult := mrOk;
          LOk.Default := True;
          LOk.Enabled := LList.Items.Count > 0;
          LCancel := TButton.Create(LDialog);
          LCancel.Parent := LDialog;
          LCancel.SetBounds(612, 346, 92, 30);
          LCancel.Caption := '取消';
          LCancel.ModalResult := mrCancel;
          LCancel.Cancel := True;
          if LList.Items.Count = 0 then
          begin
            MessageDlg(Format('没有可选窗口（%d 个窗口因提权、敏感程序或不可访问被阻止）。',
              [LBlocked]), mtWarning, [mbOK], 0);
            Exit;
          end;
          if LDialog.ShowModal <> mrOk then
            Exit;
          LRequest := TJSONObject.Create;
          try
            LRequest.AddPair('target_hwnd', LHandles[LList.ItemIndex]);
            LSelectedResponse := PostJsonWith(LClient,
              '/admin/computer/select-window', LRequest.ToJSON);
            LSelectedResponse.Free;
          finally
            LRequest.Free;
          end;
          AppendLog('已明确选择 Desktop 目标：' + LList.Items[LList.ItemIndex] +
            Format('；另有 %d 个窗口被安全策略排除。', [LBlocked]));
          finally
            LDialog.Free;
          end;
        finally
          LResponse.Free;
        end;
      finally
        LClient.Free;
      end;
      RefreshStatusAsync(True);
    except
      on E: Exception do
        MessageDlg('选择 Desktop 窗口失败：' + E.Message, mtError, [mbOK], 0);
    end;
  finally
    LHandles.Free;
  end;
end;

procedure TMainForm.ComputerAllowClick(Sender: TObject);
begin
  DecideComputerApprovalAsync(True);
end;

procedure TMainForm.ComputerRejectClick(Sender: TObject);
begin
  DecideComputerApprovalAsync(False);
end;

procedure TMainForm.DecideComputerApprovalAsync(const AApprove: Boolean);
var
  LSession: string;
  LBatch: string;
  LHash: string;
begin
  LSession := FComputerApprovalSession;
  LBatch := FComputerApprovalBatch;
  LHash := FComputerApprovalHash;
  if (LSession = '') or (LBatch = '') or (LHash = '') then
    Exit;
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LClient: THTTPClient;
      LRequest: TJSONObject;
      LResponse: TJSONObject;
      LError: string;
      LPath: string;
    begin
      LError := '';
      try
        LRequest := TJSONObject.Create;
        try
          LRequest.AddPair('session_id', LSession);
          LRequest.AddPair('batch_id', LBatch);
          LRequest.AddPair('action_hash', LHash);
          if AApprove then
            LPath := '/admin/computer/approve'
          else
            LPath := '/admin/computer/reject';
          LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
          try
            LResponse := PostJsonWith(LClient, LPath, LRequest.ToJSON);
            LResponse.Free;
          finally
            LClient.Free;
          end;
        finally
          LRequest.Free;
        end;
      except
        on E: Exception do
          LError := E.Message;
      end;
      QueueToMain(
        procedure
        begin
          if LError <> '' then
            AppendLog('Computer Use 审批失败：' + LError)
          else if AApprove then
            AppendLog('已由真实用户允许本批动作一次。')
          else
            AppendLog('已拒绝本批 Computer Use 动作。');
          RefreshStatusAsync(True);
        end);
    end).Start;
end;

procedure TMainForm.StopComputerAsync;
begin
  Inc(FWorkerCount);
  TThread.CreateAnonymousThread(
    procedure
    var
      LClient: THTTPClient;
      LResponse: TJSONObject;
      LError: string;
    begin
      LError := '';
      try
        LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
        try
          LResponse := PostJsonWith(LClient, '/admin/computer/cancel', '{}');
          LResponse.Free;
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
          if LError <> '' then
            AppendLog('Computer Use 停止失败：' + LError)
          else
            AppendLog('Computer Use 会话已立即停止。');
          RefreshStatusAsync(True);
        end);
    end).Start;
end;

procedure TMainForm.StartProfilesRefresh(const AQuiet: Boolean);
begin
  if FClosing or FProfilesBusy then
    Exit;
  FProfilesBusy := True;
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
              if SameText(LTransport, 'gemini-interactions') then
                LRow.Route := 'Gemini 原生'
              else if SameText(LTransport, 'gemini') or
                 JsonBool(LProfile, 'local_gemini') then
                LRow.Route := 'Gemini 转换'
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

procedure TMainForm.SetThinkingLevelAsync(const ALevel: string);
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
      LAppliedLevel: string;
      LError: string;
    begin
      LAppliedLevel := '';
      LError := '';
      try
        LClient := NewHttpClient(ACTION_RESPONSE_TIMEOUT_MS);
        try
          LRequest := TJSONObject.Create;
          try
            LRequest.AddPair('level', ALevel);
            LResult := PostJsonWith(
              LClient,
              '/admin/gemini-thinking-level',
              LRequest.ToJSON
            );
          finally
            LRequest.Free;
          end;
          try
            LAppliedLevel := JsonText(LResult, 'gemini_thinking_level');
          finally
            LResult.Free;
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
          SetBusy(False);
          if LError <> '' then
          begin
            AppendLog('Thinking 档位切换失败：' + LError);
            MessageDlg(LError, mtError, [mbOK], 0);
          end
          else
          begin
            FThinkingLevel := LAppliedLevel;
            AppendLog('Gemini Thinking 已切换为：' + LAppliedLevel);
          end;
          RefreshStatusAsync(True);
        end);
    end).Start;
end;

procedure TMainForm.ThinkingLevelClick(Sender: TObject);
var
  LLevel: string;
begin
  if FUpdatingThinkingUi or FBusy or not FThinkingGroup.Enabled then
    Exit;
  case FThinkingGroup.ItemIndex of
    0: LLevel := 'low';
    1: LLevel := 'medium';
    2: LLevel := 'high';
  else
    Exit;
  end;
  if SameText(LLevel, FThinkingLevel) then
    Exit;
  SetThinkingLevelAsync(LLevel);
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
  LWaitResult: DWORD;
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
    LWaitResult := WaitForSingleObject(
      LProcessInfo.hProcess,
      SCRIPT_PROCESS_TIMEOUT_MS
    );
    if LWaitResult = WAIT_FAILED then
      RaiseLastOSError;
    if LWaitResult = WAIT_TIMEOUT then
      raise Exception.Create('服务脚本运行超过 5 分钟，仍未结束。');
    if LWaitResult <> WAIT_OBJECT_0 then
      raise Exception.CreateFmt('等待服务脚本失败，状态码 %d。', [LWaitResult]);
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

procedure TMainForm.PollTimer(Sender: TObject);
begin
  RefreshStatusAsync(True);
end;

end.
