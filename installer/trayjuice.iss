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
; updates" may run a newer installer over it. Removed with the whole key
; on uninstall (see [Code]).
Root: HKCU; Subkey: "Software\TrayJuice"; ValueType: string; ValueName: "InstallDir"; ValueData: "{app}"

[Run]
; After a normal install the user decides; after an update started by the
; app (silent) it simply comes back.
Filename: "{app}\trayjuice.exe"; Description: "Start TrayJuice"; Flags: nowait postinstall skipifsilent
Filename: "{app}\trayjuice.exe"; Flags: nowait; Check: WizardSilent

[Code]
const
  // The app holds Local\TrayJuice.SingleInstance while it runs; a name
  // without a prefix is looked up in the same session namespace.
  AppMutexName = 'TrayJuice.SingleInstance';

// Waits for a copy that is on its way out (the app quits right after it
// starts an update), then asks the user to quit one that stays.
function WaitUntilClosed(): Boolean;
var
  I: Integer;
begin
  Result := True;
  for I := 1 to 25 do
  begin
    if not CheckForMutexes(AppMutexName) then
      Exit;
    Sleep(200);
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

function InitializeSetup(): Boolean;
begin
  Result := WaitUntilClosed();
end;

function InitializeUninstall(): Boolean;
begin
  Result := WaitUntilClosed();
end;

// The app writes these itself, so the installer does not know about them:
// the "Start with Windows" value and the saved menu choices.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER,
      'Software\Microsoft\Windows\CurrentVersion\Run', 'TrayJuice');
    RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER, 'Software\TrayJuice');
  end;
end;
