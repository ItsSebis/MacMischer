; Inno Setup script for the Windows Release asset (MacMischer-<ver>-windows-x64-setup.exe).
; Build: set MM_VERSION, then `iscc tools\windows\macmischer.iss` from the repo root after
; `cargo build --release -p macmischer`. Output: dist\MacMischer-<ver>-windows-x64-setup.exe.
; Per-user install (no admin prompt) so the in-app Update can run it silently. The uninstaller
; (unins000.exe) next to the exe is how the app knows it was installed by this program.
#define AppVer GetEnv("MM_VERSION")
#if AppVer == ""
  #error Set MM_VERSION
#endif

[Setup]
AppId={{6C3D5C0E-3B7E-4B52-9D0A-7A1E4F2B9C11}
AppName=MacMischer
AppVersion={#AppVer}
AppPublisher=MacMischer
DefaultDirName={autopf}\MacMischer
DefaultGroupName=MacMischer
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=MacMischer-{#AppVer}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
CloseApplications=force
RestartApplications=no
UninstallDisplayName=MacMischer
SetupIconFile=..\..\assets\icon\icon.ico
UninstallDisplayIcon={app}\MacMischer.exe

[Files]
Source: "..\..\target\release\macmischer.exe"; DestDir: "{app}"; DestName: "MacMischer.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\MacMischer"; Filename: "{app}\MacMischer.exe"

[Run]
; Silent runs are Updates started from the app: start the new version again.
Filename: "{app}\MacMischer.exe"; Flags: nowait runasoriginaluser; Check: WizardSilent
Filename: "{app}\MacMischer.exe"; Description: "Launch MacMischer"; Flags: nowait postinstall skipifsilent runasoriginaluser
