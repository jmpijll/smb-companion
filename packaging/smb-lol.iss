; SMB // League of Legends companion - per-user installer (no admin rights).
; Build: ISCC /DAppVersion=0.2.0 /DSourceExe=..\target\release\smb-lol.exe packaging\smb-lol.iss
; packaging\package.ps1 does this for you.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\smb-lol.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#define AppName "SMB LoL Companion"
#define AppExe "smb-lol.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"

[Setup]
; Never change AppId: it ties upgrades and the uninstaller together.
AppId={{6F1C2B9E-7A54-4D7F-9C2E-5B8A1D3E4F60}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=SMB - Sexy Maar Bescheiden
AppPublisherURL=https://github.com/jmpijll/smb-lol
AppSupportURL=https://github.com/jmpijll/smb-lol/issues
AppUpdatesURL=https://github.com/jmpijll/smb-lol/releases
AppCopyright=MIT License. Not endorsed by Riot Games.
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
; Per-user install into %LOCALAPPDATA%\Programs: no UAC prompt.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DisableWelcomePage=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=smb-lol-v{#AppVersion}-setup
SetupIconFile=..\assets\icon\smb-lol.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
; Our own [Code] closes the running companion first; Restart Manager is the fallback.
CloseApplications=yes
RestartApplications=no
ShowLanguageDialog=no
; Brand: Void Black, Neon Pink mark, dark controls in every Windows theme.
WizardStyle=modern dark hidebevels
WizardBackColor=#050305
WizardImageBackColor=#050305
WizardSmallImageBackColor=#050305
WizardImageFile=wizard-large-100.png,wizard-large-200.png
WizardSmallImageFile=wizard-small-100.png,wizard-small-200.png

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "nl"; MessagesFile: "compiler:Languages\Dutch.isl"

[CustomMessages]
en.StartupGroup=Startup:
nl.StartupGroup=Opstarten:
en.TaskAutostart=Start with Windows (opens in the tray, never over your game)
nl.TaskAutostart=Starten met Windows (opent in het systeemvak, nooit over je game)

[Messages]
en.WelcomeLabel2=This will install [name/ver] on your computer.%n%nRunes and summoner spells in champion select. Nothing else. It never opens over your game and lives in the tray.%n%nSMB - Sexy Maar Bescheiden.
nl.WelcomeLabel2=Hiermee wordt [name/ver] op deze computer geïnstalleerd.%n%nRunes en summoner spells in champion select. Verder niets. Opent nooit over je game en woont in het systeemvak.%n%nSMB - Sexy Maar Bescheiden.

[Tasks]
Name: "autostart"; Description: "{cm:TaskAutostart}"; GroupDescription: "{cm:StartupGroup}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Must match exactly what the app's own "Start with Windows" toggle writes.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"" --background"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

; Settings in %APPDATA%\SMB\LoL Companion are deliberately kept on uninstall:
; other copies (portable or older builds) may share them.

[Code]
// Ask a running companion (installed or portable) to exit cleanly, tray icon included.
procedure StopCompanion(const Exe: String);
var
  Code: Integer;
begin
  if FileExists(Exe) then
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  ExtractTemporaryFile('{#AppExe}');
  StopCompanion(ExpandConstant('{tmp}\{#AppExe}'));
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  StopCompanion(ExpandConstant('{app}\{#AppExe}'));
  Result := True;
end;

// Remove the start-up entry only if it points at this installation, so a
// portable copy's own setting is left alone.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Value: String;
begin
  if (CurUninstallStep = usPostUninstall) and
     RegQueryStringValue(HKCU, '{#RunKey}', '{#AppName}', Value) and
     (Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Value)) > 0) then
    RegDeleteValue(HKCU, '{#RunKey}', '{#AppName}');
end;
