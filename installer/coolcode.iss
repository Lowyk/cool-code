; Inno Setup script for the Windows installer.
; Build:  ISCC /DAppVersion=0.1.0 installer\coolcode.iss   (after `cargo build --release`)
; Output: dist\coolcode-v<version>-windows-x64-setup.exe

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\coolcode.exe"
#endif

[Setup]
AppId={{103540D2-3B5B-4976-8377-5CF8A8656DF1}
AppName=Cool Code
AppVersion={#AppVersion}
AppVerName=Cool Code {#AppVersion}
AppPublisher=Lowyk
AppPublisherURL=https://github.com/Lowyk/cool-code
AppSupportURL=https://github.com/Lowyk/cool-code/issues
AppUpdatesURL=https://github.com/Lowyk/cool-code/releases
; Per user by default (no administrator prompt); the dialog lets people choose all users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\Cool Code
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=coolcode-v{#AppVersion}-windows-x64-setup
LicenseFile=..\LICENSE
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
ChangesEnvironment=yes
UninstallDisplayName=Cool Code
UninstallDisplayIcon={app}\coolcode.exe

[Tasks]
Name: "addtopath"; Description: "Add Cool Code to PATH, so that typing coolcode works in any terminal"; Flags: checkedonce

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Opens a terminal window running Cool Code in the user's home folder.
Name: "{autoprograms}\Cool Code"; Filename: "{cmd}"; Parameters: "/k ""{app}\coolcode.exe"""; WorkingDir: "{%USERPROFILE}"; Comment: "Cool Code, an AI coding assistant for your terminal"

[Run]
Filename: "{cmd}"; Parameters: "/k ""{app}\coolcode.exe"""; WorkingDir: "{%USERPROFILE}"; Description: "Start Cool Code"; Flags: postinstall nowait skipifsilent unchecked

[Code]
function EnvironmentKey(): String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function PathContains(const Paths, Folder: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Folder) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath();
var
  Paths, Folder: String;
begin
  Folder := ExpandConstant('{app}');
  if not RegQueryStringValue(HKA, EnvironmentKey(), 'Path', Paths) then
    Paths := '';
  if PathContains(Paths, Folder) then
    Exit;
  if (Paths <> '') and (Paths[Length(Paths)] <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(HKA, EnvironmentKey(), 'Path', Paths + Folder);
end;

procedure RemoveFromPath();
var
  Paths, Folder: String;
  Position: Integer;
begin
  Folder := ExpandConstant('{app}');
  if not RegQueryStringValue(HKA, EnvironmentKey(), 'Path', Paths) then
    Exit;
  Paths := ';' + Paths + ';';
  Position := Pos(';' + Uppercase(Folder) + ';', Uppercase(Paths));
  if Position = 0 then
    Exit;
  Delete(Paths, Position, Length(Folder) + 1);
  // Drop the extra separators the wrapping added.
  if (Paths <> '') and (Paths[1] = ';') then
    Delete(Paths, 1, 1);
  if (Paths <> '') and (Paths[Length(Paths)] = ';') then
    Delete(Paths, Length(Paths), 1);
  RegWriteExpandStringValue(HKA, EnvironmentKey(), 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath();
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath();
end;
