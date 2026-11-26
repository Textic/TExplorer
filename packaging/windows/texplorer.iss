#ifndef MyAppVersion
  #define MyAppVersion "0.2.0"
#endif
#ifndef BinaryPath
  #define BinaryPath "..\..\target\release\texplorer.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
#ifndef OutputBaseFilename
  #define OutputBaseFilename "texplorer-v0.2.0-windows-x86_64-setup"
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
VersionInfoVersion={#MyAppVersion}.0

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinaryPath}"; DestDir: "{app}"; DestName: "texplorer.exe"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\texplorer.ico"; DestDir: "{app}\assets"; Flags: ignoreversion

[Icons]
Name: "{group}\TExplorer"; Filename: "{app}\texplorer.exe"; IconFilename: "{app}\texplorer.exe"
Name: "{group}\{cm:UninstallProgram,TExplorer}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\TExplorer"; Filename: "{app}\texplorer.exe"; IconFilename: "{app}\texplorer.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\texplorer.exe"; Description: "{cm:LaunchProgram,TExplorer}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Always restore original Windows Explorer HKCU folder associations on uninstall
Filename: "{app}\texplorer.exe"; Parameters: "--restore-explorer"; RunOnceId: "RestoreWindowsExplorer"; Flags: runhidden waituntilterminated
