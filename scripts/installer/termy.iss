#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

; Version as it appears in the output file name. Release builds pass the tag
; (e.g. v1.2.3) so the asset keeps its published name, while AppVersion, shown
; in Apps & features, stays a bare version.
#ifndef MyOutputVersion
  #define MyOutputVersion MyAppVersion
#endif

#ifndef MyArch
  #define MyArch "x64"
#endif

#ifndef MyTarget
  #define MyTarget "x86_64-pc-windows-msvc"
#endif

#ifndef MyExeName
  #define MyExeName "termy.exe"
#endif

#ifndef MyCliExeName
  #define MyCliExeName "termy-cli.exe"
#endif

; Must match the AppUserModelID the app sets on its windows, so pinned
; taskbar shortcuts and running windows group together.
#define MyAppUserModelID "lassejlv.Termy"

#if MyArch == "x64"
  #define MyArchAllowed "x64compatible"
  #define MyArchInstallMode "x64compatible"
#elif MyArch == "arm64"
  #define MyArchAllowed "arm64"
  #define MyArchInstallMode "arm64"
#else
  #error Unsupported MyArch value. Use x64 or arm64.
#endif

[Setup]
AppId={{7D3DD34B-5F8F-4D7B-BBC9-0F54B4C89142}
AppName=Termy
AppVersion={#MyAppVersion}
AppPublisher=Termy
AppPublisherURL=https://github.com/lassejlv/termy
AppSupportURL=https://github.com/lassejlv/termy/issues
AppUpdatesURL=https://github.com/lassejlv/termy/releases
; Per-user by default ({autopf} is %LOCALAPPDATA%\Programs), no UAC prompt.
; The dialog or /ALLUSERS installs for all users under Program Files, and
; upgrades of an existing install keep its mode (UsePreviousPrivileges).
DefaultDirName={autopf}\Termy
DefaultGroupName=Termy
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
UsedUserAreasWarning=no
OutputDir=..\..\target\dist
OutputBaseFilename=Termy-{#MyOutputVersion}-windows-{#MyArch}-Setup
SetupIconFile=..\..\assets\termy.ico
Compression=lzma
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed={#MyArchAllowed}
ArchitecturesInstallIn64BitMode={#MyArchInstallMode}
UninstallDisplayIcon={app}\{#MyExeName}
CloseApplications=yes
RestartApplications=no
; The uninstaller edits the user's Path; tell Explorer to reload it.
ChangesEnvironment=yes

[Tasks]
; Unchecked by default; Setup remembers the choice, so silent updates neither
; add a desktop shortcut nor bring back one the user deleted.
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\target\{#MyTarget}\release\{#MyExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\target\{#MyTarget}\release\{#MyCliExeName}"; DestDir: "{app}"; Flags: ignoreversion
; WinUI 2 (Microsoft.UI.Xaml) for the native tab strip, staged beside
; termy.exe by the desktop crate's build script.
Source: "..\..\target\{#MyTarget}\release\Microsoft.UI.Xaml.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\target\{#MyTarget}\release\resources.pri"; DestDir: "{app}"; Flags: ignoreversion
; Newer ConPTY than the OS one, from the Microsoft.Windows.Console.ConPTY
; NuGet package. Release CI places them beside termy.exe; local builds
; without them fall back to the system ConPTY.
Source: "..\..\target\{#MyTarget}\release\conpty.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\..\target\{#MyTarget}\release\OpenConsole.exe"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\Termy"; Filename: "{app}\{#MyExeName}"; AppUserModelID: "{#MyAppUserModelID}"
Name: "{autodesktop}\Termy"; Filename: "{app}\{#MyExeName}"; AppUserModelID: "{#MyAppUserModelID}"; Tasks: desktopicon

[Registry]
; HKA is HKLM for all-users installs and HKCU for per-user ones, so the URL
; handler and context menu live in Software\Classes of the matching hive.
Root: HKA; Subkey: "Software\Classes\termy"; ValueType: string; ValueName: ""; ValueData: "URL:Termy Protocol"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\termy"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\termy\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyExeName},0"
Root: HKA; Subkey: "Software\Classes\termy\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyExeName}"" ""%1"""
; "%V\." rather than "%V": on a drive root %V is "C:\", and the trailing
; backslash would escape the closing quote ("C:\" parses as C:").
Root: HKA; Subkey: "Software\Classes\Directory\shell\TermyOpenTab"; ValueType: string; ValueName: ""; ValueData: "Open new Termy tab here"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\TermyOpenTab"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyExeName}"
Root: HKA; Subkey: "Software\Classes\Directory\shell\TermyOpenTab\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyExeName}"" --working-directory ""%V\."""
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\TermyOpenTab"; ValueType: string; ValueName: ""; ValueData: "Open new Termy tab here"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\TermyOpenTab"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyExeName}"
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\TermyOpenTab\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyExeName}"" --working-directory ""%V\."""
Root: HKA; Subkey: "Software\Classes\Drive\shell\TermyOpenTab"; ValueType: string; ValueName: ""; ValueData: "Open new Termy tab here"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Drive\shell\TermyOpenTab"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyExeName}"
Root: HKA; Subkey: "Software\Classes\Drive\shell\TermyOpenTab\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyExeName}"" --working-directory ""%V\."""
; Lets termy-cli (and Win+R / ShellExecute) find the GUI without PATH.
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#MyExeName}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyExeName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#MyExeName}"; ValueType: string; ValueName: "Path"; ValueData: "{app}"

[Run]
Filename: "{app}\{#MyExeName}"; Description: "Launch Termy"; Flags: nowait postinstall skipifsilent
; Silent auto-updates quit Termy before setup finishes, so relaunch after install.
; runasoriginaluser keeps the relaunched app unelevated when Setup elevated
; itself for an all-users install.
Filename: "{app}\{#MyExeName}"; Flags: nowait runasoriginaluser skipifnotsilent

[UninstallRun]
; Removes the per-user Explorer and URL handler keys the app registers itself.
; [UninstallRun] cannot drop to the original user, so an elevated uninstall
; cleans the HKCU of the account that approved it (the same user under UAC
; admin approval mode).
Filename: "{app}\{#MyExeName}"; Parameters: "--unregister-file-manager-integration"; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterFileManagerIntegration"

[UninstallDelete]
; The per-user command-line shim `termy install-cli` places on PATH.
Type: filesandordirs; Name: "{localappdata}\Termy\bin"

[Code]
function IsCliBinPathEntry(Entry: String): Boolean;
var
  Normalized: String;
begin
  Normalized := RemoveBackslashUnlessRoot(Trim(Entry));
  Result :=
    (CompareText(Normalized, RemoveBackslashUnlessRoot(ExpandConstant('{localappdata}\Termy\bin'))) = 0) or
    (CompareText(Normalized, '%LOCALAPPDATA%\Termy\bin') = 0);
end;

// Drops the CLI shim directory from the user's Path, leaving every other
// entry (including empty ones) exactly as it was.
procedure RemoveCliBinFromUserPath;
var
  OldPath, Remaining, Entry, NewPath: String;
  Separator: Integer;
  Removed, First: Boolean;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OldPath) then
    Exit;

  NewPath := '';
  Remaining := OldPath;
  Removed := False;
  First := True;
  while True do
  begin
    Separator := Pos(';', Remaining);
    if Separator = 0 then
      Entry := Remaining
    else
      Entry := Copy(Remaining, 1, Separator - 1);

    if (Entry <> '') and IsCliBinPathEntry(Entry) then
      Removed := True
    else
    begin
      if First then
        NewPath := Entry
      else
        NewPath := NewPath + ';' + Entry;
      First := False;
    end;

    if Separator = 0 then
      Break;
    Remaining := Copy(Remaining, Separator + 1, Length(Remaining));
  end;

  if Removed then
    RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', NewPath);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveCliBinFromUserPath;
end;
