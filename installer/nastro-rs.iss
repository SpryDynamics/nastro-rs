; Inno Setup script for the nastro-rs Windows installer. Build it with eng/build-installer.ps1.
;
; Defaults to a per-user install (no administrator rights, nothing outside the user's profile). Running the setup as
; administrator, or choosing "all users" in the dialog, installs to Program Files and edits the machine PATH instead.
;
; Defines (passed by eng/build-installer.ps1):  AppVersion  BinDir  OutputDir  [Sign]

#ifndef AppVersion
  #error AppVersion must be defined (e.g. /DAppVersion=0.1.1)
#endif
#ifndef BinDir
  #error BinDir must be defined (the folder holding nastro.exe and nastro-tap.exe)
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

#define AppName "nastro-rs"
#define Publisher "Spry Dynamics, LLC"

[Setup]
; Never change AppId: it is how upgrades and uninstall find the existing installation.
AppId={{4801EF68-4D86-4FD0-A4F2-21F77B9B5957}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#Publisher}
AppPublisherURL=https://github.com/SpryDynamics/nastro-rs
AppSupportURL=https://github.com/SpryDynamics/nastro-rs/issues
AppUpdatesURL=https://github.com/SpryDynamics/nastro-rs/releases
AppCopyright=Copyright (c) 2026 {#Publisher}
VersionInfoVersion={#AppVersion}.0
VersionInfoDescription={#AppName} setup
VersionInfoProductName={#AppName}

PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

LicenseFile=..\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename={#AppName}-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
; PATH changes must reach new terminals without a logoff.
ChangesEnvironment=yes
; If nastro is open (it is a long-running TUI), offer to close it rather than failing on a locked file.
CloseApplications=yes
UninstallDisplayName={#AppName}
#ifdef Sign
SignTool=azsign
SignedUninstaller=yes
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "addtopath"; Description: "Add nastro to the PATH, so you can type ""nastro"" in any terminal"; GroupDescription: "Command line:"
Name: "startmenu"; Description: "Create a Start menu shortcut"; GroupDescription: "Shortcuts:"

[Files]
; nastro finds its capture helper next to itself, so the two must stay together.
Source: "{#BinDir}\nastro.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\nastro-tap.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\NOTICE.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Opens a console window with the interface, for people who do not start from a terminal.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\nastro.exe"; Comment: "Record and transcribe calls"; Tasks: startmenu

[Code]
const
  UserEnvKey = 'Environment';
  MachineEnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

function ReadPath(var Value: string): Boolean;
begin
  if IsAdminInstallMode then
    Result := RegQueryStringValue(HKLM, MachineEnvKey, 'Path', Value)
  else
    Result := RegQueryStringValue(HKCU, UserEnvKey, 'Path', Value);
end;

procedure WritePath(const Value: string);
begin
  { Keep it expandable: PATH usually contains %VARIABLES%. }
  if IsAdminInstallMode then
    RegWriteExpandStringValue(HKLM, MachineEnvKey, 'Path', Value)
  else
    RegWriteExpandStringValue(HKCU, UserEnvKey, 'Path', Value);
end;

function Trimmed(const S: string): string;
begin
  Result := Trim(S);
  while (Length(Result) > 3) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

function SameDir(const A, B: string): Boolean;
begin
  Result := CompareText(Trimmed(A), Trimmed(B)) = 0;
end;

{ Splits the next ';'-separated item off Rest. }
function NextItem(var Rest: string): string;
var
  P: Integer;
begin
  P := Pos(';', Rest);
  if P = 0 then
  begin
    Result := Rest;
    Rest := '';
  end
  else
  begin
    Result := Copy(Rest, 1, P - 1);
    Rest := Copy(Rest, P + 1, MaxInt);
  end;
end;

function PathContains(const Paths, Dir: string): Boolean;
var
  Rest: string;
begin
  Result := False;
  Rest := Paths;
  while Rest <> '' do
    if SameDir(NextItem(Rest), Dir) then
    begin
      Result := True;
      Exit;
    end;
end;

{ Every item except those equal to Dir, in their original order and spelling. }
function PathWithout(const Paths, Dir: string): string;
var
  Rest, Item: string;
begin
  Result := '';
  Rest := Paths;
  while Rest <> '' do
  begin
    Item := NextItem(Rest);
    if not SameDir(Item, Dir) then
    begin
      if Result <> '' then Result := Result + ';';
      Result := Result + Item;
    end;
  end;
end;

procedure AddToPath(const Dir: string);
var
  Paths: string;
begin
  if not ReadPath(Paths) then Paths := '';
  if PathContains(Paths, Dir) then Exit; { upgrade or reinstall: do not duplicate }
  if (Paths <> '') and (Paths[Length(Paths)] <> ';') then Paths := Paths + ';';
  WritePath(Paths + Dir);
end;

procedure RemoveFromPath(const Dir: string);
var
  Paths: string;
begin
  if ReadPath(Paths) and PathContains(Paths, Dir) then
    WritePath(PathWithout(Paths, Dir));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;

{ nastro needs ffmpeg at run time (it is not bundled); say so on the last page if it is not on PATH. }
procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpFinished) and (FileSearch('ffmpeg.exe', GetEnv('PATH')) = '') then
    WizardForm.FinishedLabel.Caption := WizardForm.FinishedLabel.Caption + #13#10#13#10 +
      'nastro also needs FFmpeg, which is not installed. Get it with:' + #13#10 +
      '    winget install Gyan.FFmpeg' + #13#10 +
      'Open a new terminal afterwards (and after this setup, for the PATH change to apply).';
end;
