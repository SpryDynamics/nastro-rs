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
Name: "ffmpeg"; Description: "Download and install FFmpeg with winget (nastro needs it to record and transcribe)"; GroupDescription: "Tools nastro needs:"; Check: FfmpegMissing
Name: "whisperx"; Description: "Download and install speaker labels (WhisperX: about 2 GB, plus a free Hugging Face account)"; GroupDescription: "Optional tools:"; Flags: unchecked

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

{ ---------------------------------------------------------------------------------------------- optional tools
  Everything below is opt-in and user-level (winget and uv install into the user's profile). A failure never fails
  the installation of nastro itself: it ends up as a note on the last page with what to do by hand. }

var
  ModelPage: TInputOptionWizardPage;
  TokenPage: TWizardPage;
  TokenEdit: TPasswordEdit;
  FinishNotes: string;
  FfmpegAttempted: Boolean;

const
  ModelCount = 4;

function ModelName(Index: Integer): string;
begin
  case Index of
    0: Result := 'large-v3-turbo';
    1: Result := 'medium';
    2: Result := 'small';
  else
    Result := 'base';
  end;
end;

procedure AddNote(const Text: string);
begin
  if FinishNotes <> '' then FinishNotes := FinishNotes + #13#10#13#10;
  FinishNotes := FinishNotes + Text;
end;

{ The /TASKS= switch of a scripted install, with quotes removed. }
function TasksSwitchHas(const Name: string): Boolean;
var
  I: Integer;
  P: string;
begin
  Result := False;
  for I := 1 to ParamCount do
  begin
    P := ParamStr(I);
    if CompareText(Copy(P, 1, 7), '/TASKS=') = 0 then
    begin
      StringChangeEx(P, '"', '', True);
      if Pos(',' + Lowercase(Name) + ',', ',' + Lowercase(Copy(P, 8, MaxInt)) + ',') > 0 then
        Result := True;
    end;
  end;
end;

{ Interactive: whatever is ticked. Silent: only what /TASKS= names, so a plain /VERYSILENT install never touches the
  network (a task that is ticked by default would otherwise run). }
function Wanted(const Task: string): Boolean;
begin
  Result := WizardIsTaskSelected(Task) and ((not WizardSilent) or TasksSwitchHas(Task));
end;

function WingetLinks(): string;
begin
  Result := ExpandConstant('{localappdata}\Microsoft\WinGet\Links');
end;

{ On PATH, or where winget links its tools (a PATH change made after this setup started is not visible to it). }
function ToolPresent(const Exe: string): Boolean;
begin
  Result := (FileSearch(Exe, GetEnv('PATH')) <> '') or FileExists(WingetLinks() + '\' + Exe);
end;

function FfmpegMissing: Boolean;
begin
  Result := (GetEnv('NASTRO_FFMPEG') = '') and not ToolPresent('ffmpeg.exe');
end;

procedure SetStatus(const Text: string);
begin
  WizardForm.StatusLabel.Caption := Text;
  WizardForm.StatusLabel.Update;
end;

{ Runs `cmd /c <Line>` as the user who started setup (not the elevated account, if any). }
function RunCmd(const Line: string; Show: Integer; var Code: Integer): Boolean;
begin
  Result := ExecAsOriginalUser(ExpandConstant('{cmd}'), '/c ' + Line, '', Show, ewWaitUntilTerminated, Code);
end;

function WingetAvailable: Boolean;
var
  Code: Integer;
begin
  Result := RunCmd('winget --version', SW_HIDE, Code) and (Code = 0);
end;

function WingetInstall(const Id: string; var Code: Integer): Boolean;
begin
  Result := RunCmd('winget install --id ' + Id + ' -e --silent --disable-interactivity ' +
    '--accept-package-agreements --accept-source-agreements', SW_HIDE, Code);
end;

procedure InstallFfmpeg;
var
  Code: Integer;
begin
  FfmpegAttempted := True;
  SetStatus('Installing FFmpeg with winget...');
  if not WingetAvailable then
  begin
    AddNote('FFmpeg was not installed: winget is not available on this computer. Install FFmpeg yourself (for example ' +
      'from https://www.gyan.dev/ffmpeg/builds/) and make sure ffmpeg.exe is on your PATH.');
    Exit;
  end;
  if (WingetInstall('Gyan.FFmpeg', Code) and (Code = 0)) or ToolPresent('ffmpeg.exe') then
    AddNote('FFmpeg is installed. Open a new terminal before using nastro, so that it is found.')
  else
    AddNote('FFmpeg could not be installed automatically (winget returned ' + IntToStr(Code) + '). Install it with:' + #13#10 + '    winget install Gyan.FFmpeg');
end;

function InstallWhisperx: Boolean;
var
  Code, Show: Integer;
  Nastro, Line: string;
begin
  Result := False;
  Nastro := ExpandConstant('{app}\nastro.exe');
  if not (ToolPresent('uv.exe') or FileExists(ExpandConstant('{%USERPROFILE}\.local\bin\uv.exe'))) then
  begin
    SetStatus('Installing uv (the tool that installs WhisperX) with winget...');
    if not WingetAvailable then
    begin
      AddNote('Speaker labels were not installed: they need the "uv" tool and winget is not available to install it. ' +
        'Install uv from https://docs.astral.sh/uv/ and then run:' + #13#10 + '    nastro transcribe --setup-whisperx');
      Exit;
    end;
    if not (WingetInstall('astral-sh.uv', Code) and (Code = 0)) then
    begin
      AddNote('Speaker labels were not installed: "uv" could not be installed (winget returned ' + IntToStr(Code) +
        '). Install it with "winget install astral-sh.uv", then run:' + #13#10 + '    nastro transcribe --setup-whisperx');
      Exit;
    end;
  end;

  SetStatus('Installing speaker labels (WhisperX). A console window shows the progress; this takes a while...');
  Line := '""' + Nastro + '" transcribe --setup-whisperx';
  if WizardSilent then
  begin
    Show := SW_HIDE;
    Line := Line + '"';
  end
  else
  begin
    Show := SW_SHOW;
    Line := Line + ' || (echo. & echo The speaker-labels setup did not finish. & ' +
      'echo You can retry later with: nastro transcribe --setup-whisperx & pause & exit /b 1)"';
  end;
  if RunCmd(Line, Show, Code) and (Code = 0) then
    Result := True
  else
    AddNote('The speaker-labels (WhisperX) setup did not finish. Retry it any time with:' + #13#10 +
      '    nastro transcribe --setup-whisperx');
end;

procedure SaveModel(const Model: string);
var
  Code: Integer;
begin
  if not (RunCmd('""' + ExpandConstant('{app}\nastro.exe') + '" config set whisper_model ' + Model + '"', SW_HIDE, Code) and (Code = 0)) then
    AddNote('The speech model could not be saved to your settings. Set it with:' + #13#10 +
      '    nastro config set whisper_model ' + Model);
end;

{ The token goes to nastro through a short-lived file that nastro deletes as soon as it has read it: Exec cannot pipe
  into a program, and a command line would be visible to other users. It is never logged or shown. }
procedure SaveToken(const Token: string);
var
  Code: Integer;
  HandOff, Nastro: string;
begin
  Nastro := ExpandConstant('{app}\nastro.exe');
  HandOff := GenerateUniqueName(ExpandConstant('{tmp}'), '.txt');
  if not SaveStringToFile(HandOff, Token + #13#10, False) then
  begin
    AddNote('Your Hugging Face token could not be saved. Add it with:' + #13#10 + '    echo hf_... | nastro config set hf_token');
    Exit;
  end;
  SetStatus('Saving your Hugging Face token...');
  if not (RunCmd('""' + Nastro + '" config set hf_token --from-file "' + HandOff + '""', SW_HIDE, Code) and (Code = 0)) then
  begin
    DeleteFile(HandOff);
    AddNote('Your Hugging Face token was not accepted (it should start with hf_). Add it with:' + #13#10 +
      '    echo hf_... | nastro config set hf_token');
    Exit;
  end;
  DeleteFile(HandOff);

  SetStatus('Checking the token with Hugging Face...');
  if not RunCmd('""' + Nastro + '" config check-hf"', SW_HIDE, Code) then Code := 4;
  case Code of
    0: AddNote('Your Hugging Face token is saved and works for speaker labels.');
    2: AddNote('Hugging Face does not accept that token. Create a new "Read" token at ' +
         'https://huggingface.co/settings/tokens and save it with:' + #13#10 + '    echo hf_... | nastro config set hf_token');
    3: AddNote('Your token is saved, but its account has not accepted the speaker model''s terms yet. ' +
         'Open https://huggingface.co/pyannote/speaker-diarization-community-1 while signed in and accept them.');
  else
    AddNote('Your token is saved. It could not be checked right now (no connection?); ' +
      'check it later with: nastro config check-hf');
  end;
end;

function ChosenModel: string;
var
  I: Integer;
  Wanted_: string;
begin
  Result := '';
  Wanted_ := ExpandConstant('{param:Model|}'); { scripted installs: /MODEL=small }
  if Wanted_ <> '' then
  begin
    for I := 0 to ModelCount - 1 do
      if CompareText(Wanted_, ModelName(I)) = 0 then Result := ModelName(I);
    Exit;
  end;
  for I := 0 to ModelCount - 1 do
    if ModelPage.Values[I] then Result := ModelName(I);
end;

procedure OpenUrl(const Url: string);
var
  Code: Integer;
begin
  ShellExec('open', Url, '', '', SW_SHOWNORMAL, ewNoWait, Code);
end;

procedure OpenTokensClick(Sender: TObject);
begin
  OpenUrl('https://huggingface.co/settings/tokens');
end;

procedure OpenTermsClick(Sender: TObject);
begin
  OpenUrl('https://huggingface.co/pyannote/speaker-diarization-community-1');
end;

procedure InitializeWizard;
var
  Steps, Prompt: TNewStaticText;
  B1, B2: TNewButton;
begin
  ModelPage := CreateInputOptionPage(wpSelectTasks, 'Speech model',
    'Which speech model should nastro use?',
    'Bigger models are more accurate but slower and use more memory. The model is downloaded the first time you ' +
    'transcribe. You can change it later in the settings file.', True, False);
  ModelPage.Add('large-v3-turbo: best accuracy for its speed (about 1.6 GB download, 2.5 GB of memory) - recommended');
  ModelPage.Add('medium: accurate, slower (about 1.5 GB)');
  ModelPage.Add('small: good for clear audio, quick (about 0.5 GB)');
  ModelPage.Add('base: fastest and smallest, least accurate (about 0.15 GB)');
  ModelPage.Values[0] := True;

  TokenPage := CreateCustomPage(ModelPage.ID, 'Hugging Face access',
    'Speaker labels need a free Hugging Face account (this step is optional)');
  Steps := TNewStaticText.Create(TokenPage);
  Steps.Parent := TokenPage.Surface;
  Steps.AutoSize := False;
  Steps.WordWrap := True;
  Steps.Left := 0;
  Steps.Top := 0;
  Steps.Width := TokenPage.SurfaceWidth;
  Steps.Height := ScaleY(96);
  Steps.Caption :=
    'The speaker models are gated, so Hugging Face needs to know who you are:' + #13#10 +
    '  1. Sign in to (or create) a free account at huggingface.co.' + #13#10 +
    '  2. Create an access token of type "Read" (first button).' + #13#10 +
    '  3. Open the speaker model page and accept its terms (second button).' + #13#10 +
    '  4. Paste the token below, or leave it empty and add it later with:  nastro config set hf_token';

  B1 := TNewButton.Create(TokenPage);
  B1.Parent := TokenPage.Surface;
  B1.Left := 0;
  B1.Top := Steps.Top + Steps.Height + ScaleY(4);
  B1.Width := ScaleX(190);
  B1.Height := ScaleY(23);
  B1.Caption := 'Create a token...';
  B1.OnClick := @OpenTokensClick;

  B2 := TNewButton.Create(TokenPage);
  B2.Parent := TokenPage.Surface;
  B2.Left := B1.Left + B1.Width + ScaleX(8);
  B2.Top := B1.Top;
  B2.Width := ScaleX(190);
  B2.Height := ScaleY(23);
  B2.Caption := 'Accept the model terms...';
  B2.OnClick := @OpenTermsClick;

  Prompt := TNewStaticText.Create(TokenPage);
  Prompt.Parent := TokenPage.Surface;
  Prompt.Left := 0;
  Prompt.Top := B1.Top + B1.Height + ScaleY(16);
  Prompt.Caption := 'Access token (it is stored in your nastro settings file for your user only):';
  Prompt.AutoSize := True;

  TokenEdit := TPasswordEdit.Create(TokenPage);
  TokenEdit.Parent := TokenPage.Surface;
  TokenEdit.Left := 0;
  TokenEdit.Top := Prompt.Top + Prompt.Height + ScaleY(4);
  TokenEdit.Width := TokenPage.SurfaceWidth;
end;

{ The two pages only make sense when speaker labels are being installed. }
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  if (PageID = ModelPage.ID) or (PageID = TokenPage.ID) then
    Result := not WizardIsTaskSelected('whisperx');
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  T: string;
begin
  Result := True;
  if CurPageID = TokenPage.ID then
  begin
    T := Trim(TokenEdit.Text);
    if (T <> '') and ((Copy(T, 1, 3) <> 'hf_') or (Length(T) < 10)) then
    begin
      MsgBox('That does not look like a Hugging Face token: they start with "hf_". ' +
        'Copy it again from huggingface.co/settings/tokens, or leave the box empty to add it later.', mbError, MB_OK);
      Result := False;
    end;
  end;
end;

{ The token: typed on the wizard page, or for scripted installs the first line of the file named by /HFTOKENFILE=
  (a path, so that the token itself never appears on a command line). That file is left alone. }
function TokenFromSetup: string;
var
  Lines: TArrayOfString;
  Path: string;
begin
  Result := '';
  if not WizardSilent then
  begin
    Result := Trim(TokenEdit.Text);
    Exit;
  end;
  Path := ExpandConstant('{param:HfTokenFile|}');
  if (Path <> '') and LoadStringsFromFile(Path, Lines) and (GetArrayLength(Lines) > 0) then
    Result := Trim(Lines[0]);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Model, Token: string;
begin
  if CurStep <> ssPostInstall then Exit;

  if WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));

  if Wanted('ffmpeg') then InstallFfmpeg;

  if Wanted('whisperx') then
  begin
    Model := ChosenModel;
    if Model <> '' then SaveModel(Model)
    else if WizardSilent then AddNote('The /MODEL= value is not one of: large-v3-turbo, medium, small, base. The model was left unchanged.');
    if InstallWhisperx then
    begin
      Token := TokenFromSetup;
      if Token <> '' then SaveToken(Token)
      else AddNote('Speaker labels are installed. They also need a Hugging Face token and accepting the speaker model''s terms. ' +
        'See "Speaker labels" in the README, or run:' + #13#10 + '    nastro config check-hf');
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;

{ The last page: what happened with the optional tools, and what is still missing. }
procedure CurPageChanged(CurPageID: Integer);
var
  Text: string;
begin
  if CurPageID <> wpFinished then Exit;
  Text := WizardForm.FinishedLabel.Caption;
  if FinishNotes <> '' then Text := Text + #13#10#13#10 + FinishNotes;
  if (not FfmpegAttempted) and FfmpegMissing then
    Text := Text + #13#10#13#10 + 'nastro also needs FFmpeg, which is not installed. Get it with:' + #13#10 +
      '    winget install Gyan.FFmpeg' + #13#10 +
      'Open a new terminal afterwards (and after this setup, for the PATH change to apply).';
  WizardForm.FinishedLabel.Caption := Text;
end;
