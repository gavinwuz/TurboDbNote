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

#ifndef AppFlavor
  #define AppFlavor "full"
#endif

[Setup]
AppId={{48E57826-489A-4C7B-A181-FBA086AB71D6}
AppName=TurboDBNote
AppVersion={#AppVersion}
AppVerName=TurboDBNote {#AppVersion}
VersionInfoVersion={#AppVersion}
; Preserve the installation location and running-app identity for upgrades.
DefaultDirName={localappdata}\Programs\TurboDbNote
DefaultGroupName=TurboDBNote
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
#if AppFlavor == "minimal"
OutputBaseFilename=TurboDBNote-v{#AppVersion}-windows-x64-minimal-setup
#else
OutputBaseFilename=TurboDBNote-v{#AppVersion}-windows-x64-setup
#endif
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\turbodbnote.exe
LicenseFile={#SourceDir}\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
DisableWelcomePage=no
WizardImageFile=assets/update-wizard.bmp,assets/update-wizard@2x.bmp,assets/update-wizard@3x.bmp
WizardSmallImageFile=assets/update-small.bmp,assets/update-small@2x.bmp,assets/update-small@3x.bmp
WizardImageStretch=yes
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
Name: "{group}\TurboDBNote"; Filename: "{app}\turbodbnote.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\TurboDBNote"; Filename: "{app}\turbodbnote.exe"; WorkingDir: "{app}"; Tasks: desktopicon

; No automatic launch and no deletion of user data directories on uninstall.

[Code]
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  { Updates retain the explicitly frozen installation directory. }
  Result := (ExpandConstant('{param:TURBODBNUPDATE|0}') = '1') and
    (PageID = wpSelectDir);
end;

procedure InitializeWizard;
var
  TextLeft, TextRight: Integer;
begin
  { Keep the application logo to the left of all standard page headings. }
  WizardForm.WizardSmallBitmapImage.Left := ScaleX(12);
  TextLeft := WizardForm.WizardSmallBitmapImage.Left +
    WizardForm.WizardSmallBitmapImage.Width + ScaleX(12);
  TextRight := WizardForm.MainPanel.ClientWidth - ScaleX(12);
  WizardForm.PageNameLabel.Left := TextLeft;
  WizardForm.PageNameLabel.Width := TextRight - TextLeft;
  WizardForm.PageDescriptionLabel.Left := TextLeft;
  WizardForm.PageDescriptionLabel.Width := TextRight - TextLeft;
end;
