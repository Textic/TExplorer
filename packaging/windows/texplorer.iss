#ifndef MyAppVersion
  #define MyAppVersion "0.6.0"
#endif
#ifndef BinaryPath
  #define BinaryPath "..\..\target\release\texplorer.exe"
#endif
#ifndef ServiceBinaryPath
  #define ServiceBinaryPath "..\..\target\release\texplorer-service.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
#ifndef OutputBaseFilename
  #define OutputBaseFilename "texplorer-v0.6.0-windows-x86_64-setup"
#endif

[Setup]
AppId={{A7C91E42-3D58-4B9E-8F14-92B6D3C0E7A1}
AppName=TExplorer
AppVersion={#MyAppVersion}
AppVerName=TExplorer {#MyAppVersion}
AppPublisher=Textic
AppPublisherURL=https://github.com/Textic/TExplorer
AppSupportURL=https://github.com/Textic/TExplorer/releases
DefaultDirName={autopf}\TExplorer
DefaultGroupName=TExplorer
AllowNoIcons=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
SetupIconFile=..\..\assets\texplorer.ico
UninstallDisplayIcon={app}\texplorer.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
VersionInfoVersion={#MyAppVersion}.0

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "defaultexplorer"; Description: "Set TExplorer as Default Windows File Manager (reversible, HKCU)"; GroupDescription: "Windows Integration:"; Flags: unchecked
Name: "defaultexplorer\wine"; Description: "Also open TExplorer when pressing Win+E"; GroupDescription: "Windows Integration:"; Flags: unchecked
Name: "mftservice"; Description: "Install NTFS $MFT Instant Search Service (requires Administrator UAC)"; GroupDescription: "Windows Integration:"; Flags: unchecked

[Files]
Source: "{#BinaryPath}"; DestDir: "{app}"; DestName: "texplorer.exe"; Flags: ignoreversion
Source: "{#ServiceBinaryPath}"; DestDir: "{app}"; DestName: "texplorer-service.exe"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\texplorer.ico"; DestDir: "{app}\assets"; Flags: ignoreversion

[Icons]
Name: "{group}\TExplorer"; Filename: "{app}\texplorer.exe"; IconFilename: "{app}\texplorer.exe"
Name: "{group}\{cm:UninstallProgram,TExplorer}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\TExplorer"; Filename: "{app}\texplorer.exe"; IconFilename: "{app}\texplorer.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\texplorer-service.exe"; Parameters: "--restart-if-installed"; Flags: runhidden waituntilterminated skipifsilent
Filename: "{app}\texplorer-service.exe"; Parameters: "--install"; Verb: "runas"; Tasks: mftservice; Flags: shellexec runhidden waituntilterminated skipifsilent
Filename: "{app}\texplorer.exe"; Parameters: "--enable-default-explorer"; Tasks: defaultexplorer and not defaultexplorer\wine; Flags: runhidden waituntilterminated
Filename: "{app}\texplorer.exe"; Parameters: "--enable-default-explorer-win-e"; Tasks: defaultexplorer\wine; Flags: runhidden waituntilterminated
Filename: "{app}\texplorer.exe"; Description: "{cm:LaunchProgram,TExplorer}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Always stop/remove the optional MFT Index Service and restore Windows Explorer HKCU associations on uninstall
Filename: "{app}\texplorer-service.exe"; Parameters: "--uninstall"; RunOnceId: "RemoveIndexService"; Flags: runhidden waituntilterminated
Filename: "{app}\texplorer.exe"; Parameters: "--restore-explorer"; RunOnceId: "RestoreWindowsExplorer"; Flags: runhidden waituntilterminated

[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExePath: String;
  ResultCode: Integer;
begin
  Result := '';
  ExePath := ExpandConstant('{app}\texplorer.exe');
  if FileExists(ExePath) then
  begin
    Exec(ExePath, '--stop-win-e-daemon', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end;
end;
