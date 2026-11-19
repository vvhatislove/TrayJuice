; Inno Setup script of the TrayJuice installer. Build it with
; installer\build.ps1, which passes the version and the executable.
;
; The installer needs no administrator rights: everything goes under the
; user's own profile and HKCU.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z (installer\build.ps1 does)
#endif
#ifndef AppExe
  #define AppExe "..\target\release\trayjuice.exe"
#endif

[Setup]
; Never change the AppId: it is how a new version finds the old one.
AppId={{6B0F0A4E-6C0D-4C1B-9E55-2A7F5D3C8B14}
AppName=TrayJuice
AppVersion={#AppVersion}
AppPublisher=Vladislav Petrenko
AppPublisherURL=https://github.com/vvhatislove/TrayJuice
AppSupportURL=https://github.com/vvhatislove/TrayJuice/issues
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\TrayJuice
DisableProgramGroupPage=yes
DisableDirPage=auto
LicenseFile=..\LICENSE
UninstallDisplayIcon={app}\trayjuice.exe
OutputDir=..\dist
; The app's "Check for updates" downloads the file by this name.
OutputBaseFilename=TrayJuice-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; The running copy is handled in [Code]: it has no window to close.
CloseApplications=no

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\TrayJuice"; Filename: "{app}\trayjuice.exe"

[Registry]
; Tells the app that this copy came from the installer, so that "Check for
; updates" may run a newer installer over it. Removed on uninstall (see
; [Code]).
Root: HKCU; Subkey: "Software\TrayJuice"; ValueType: string; ValueName: "InstallDir"; ValueData: "{app}"

[Run]
; After a normal install the user decides. When a running copy was closed
; for a silent install (an update started from the app, or a package
; manager), it simply comes back.
Filename: "{app}\trayjuice.exe"; Description: "Start TrayJuice"; Flags: nowait postinstall skipifsilent
Filename: "{app}\trayjuice.exe"; Flags: nowait; Check: RestartQuietly

[Code]
const
  // The app holds Local\TrayJuice.SingleInstance while it runs and waits
  // on the event Local\TrayJuice.Quit (src\quit_signal.rs). A name without
  // a prefix is looked up in the same session namespace.
  AppMutexName = 'TrayJuice.SingleInstance';
  QuitEventName = 'TrayJuice.Quit';
  EVENT_MODIFY_STATE = $0002;
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  SettingsKey = 'Software\TrayJuice';

var
  // A copy was running when the files were about to be replaced.
  WasRunning: Boolean;
  // The files were replaced.
  Installed: Boolean;

function TjOpenEvent(DesiredAccess: DWORD; InheritHandle: BOOL; Name: String): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function TjSetEvent(Event: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function TjCloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

// Asks a running copy to quit the way its own menu item does. Returns
// False when nothing listens: versions before 0.1.2 have to be closed by
// hand.
function AskToQuit(): Boolean;
var
  Event: THandle;
begin
  Event := TjOpenEvent(EVENT_MODIFY_STATE, False, QuitEventName);
  Result := Event <> 0;
  if Result then
  begin
    TjSetEvent(Event);
    TjCloseHandle(Event);
  end;
end;

// Closes a running copy: asks it to quit and gives it up to 3 seconds,
// then asks the user. With message boxes suppressed the answer is
// "cancel". The wait is short and only made for a copy that listens: the
// wizard's window does not respond during it (a 10 second wait made the
// installer look hung).
function CloseRunningCopy(): Boolean;
var
  I: Integer;
begin
  Result := True;
  if not CheckForMutexes(AppMutexName) then
    Exit;
  WasRunning := True;
  if AskToQuit() then
  begin
    for I := 1 to 30 do
    begin
      if not CheckForMutexes(AppMutexName) then
        Exit;
      Sleep(100);
    end;
  end;
  while CheckForMutexes(AppMutexName) do
  begin
    if SuppressibleMsgBox(
      'TrayJuice is running.' + #13#10 + #13#10 +
      'Right-click its tray icon, choose "Quit TrayJuice", then click OK.',
      mbError, MB_OKCANCEL, IDCANCEL) = IDCANCEL then
    begin
      Result := False;
      Exit;
    end;
  end;
end;

// Runs right before the files are replaced, so a wizard that is cancelled
// earlier leaves the running copy alone.
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  if CloseRunningCopy() then
    Result := ''
  else
    Result := 'TrayJuice is still running. Quit it from its tray icon menu and run the installer again.';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    Installed := True;
end;

function RestartQuietly(): Boolean;
begin
  Result := WasRunning and WizardSilent;
end;

// A copy that was closed for an install that then failed is started again
// (the old files are still there), so the user is not left without it.
procedure DeinitializeSetup();
var
  Exe: String;
  ResultCode: Integer;
begin
  if WasRunning and not Installed then
  begin
    try
      Exe := ExpandConstant('{app}\trayjuice.exe');
      if FileExists(Exe) then
        Exec(Exe, '', '', SW_SHOWNORMAL, ewNoWait, ResultCode);
    except
    end;
  end;
end;

function InitializeUninstall(): Boolean;
begin
  Result := CloseRunningCopy();
end;

// The app writes the "Start with Windows" value and the saved menu choices
// itself, so the installer has to clean them up here. They are left alone
// when "Start with Windows" points at another copy (from the zip or a
// package manager): that copy is still in use.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Command: String;
  OtherCopy: Boolean;
begin
  if CurUninstallStep = usUninstall then
  begin
    OtherCopy := RegQueryStringValue(HKEY_CURRENT_USER, RunKey, 'TrayJuice', Command)
      and (Pos(Lowercase(ExpandConstant('{app}\')), Lowercase(Command)) = 0);
    if OtherCopy then
      RegDeleteValue(HKEY_CURRENT_USER, SettingsKey, 'InstallDir')
    else
    begin
      RegDeleteValue(HKEY_CURRENT_USER, RunKey, 'TrayJuice');
      RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER, SettingsKey);
    end;
  end;
end;
