program ClaudeBridgeManager;

{$APPTYPE GUI}
{$R *.res}

uses
  Vcl.Forms,
  BridgeManagerMain in 'BridgeManagerMain.pas';

begin
  Application.Initialize;
  Application.MainFormOnTaskbar := True;
  Application.Title := 'Claude Code Model Switcher';
  Application.CreateForm(TMainForm, MainForm);
  Application.Run;
end.
