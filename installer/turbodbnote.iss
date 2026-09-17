#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef SourceDir
  #error SourceDir is required
#endif
#ifndef OutputDir
  #error OutputDir is required
#endif
#ifndef IconFile
  #error IconFile is required
#endif

[Setup]
AppId={{48E57826-489A-4C7B-A181-FBA086AB71D6}
AppName=TurboDbNote
AppVersion={#AppVersion}
AppVerName=TurboDbNote {#AppVersion}
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\TurboDbNote
DefaultGroupName=TurboDbNote
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename=TurboDbNote-v{#AppVersion}-windows-x64-setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\turbodbnote.exe
LicenseFile={#SourceDir}\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=no
RestartApplications=no
AppMutex=TurboDbNote.Running

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimp"; MessagesFile: "ChineseSimplified.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Excludes: "portable.flag"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\TurboDbNote"; Filename: "{app}\turbodbnote.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\TurboDbNote"; Filename: "{app}\turbodbnote.exe"; WorkingDir: "{app}"; Tasks: desktopicon

; No automatic launch and no deletion of user data directories on uninstall.
