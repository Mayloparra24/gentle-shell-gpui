; Windows installer for Gentle Shell (Inno Setup 6). Per-user: no admin rights,
; installs into %LOCALAPPDATA%\Programs\Gentle Shell, so the in-app updater can
; replace the exe there. Built by the Release workflow:
;   ISCC /DAppVersion=<version> /DSourceExe=<path to gentle-shell-gpui.exe>
;        /DOutputDir=<dir> /DIconFile=<path to icon.ico> installer.iss
; Output: gentle-shell-gpui-<version>-windows-x86_64-setup.exe

#ifndef AppVersion
  #error AppVersion is required (/DAppVersion=...)
#endif

[Setup]
; Never change: Windows matches upgrades and the uninstaller by this id.
AppId={{EA8317C7-3FC6-4F38-B4D6-C32357A71260}
AppName=Gentle Shell
AppVersion={#AppVersion}
AppVerName=Gentle Shell {#AppVersion}
AppPublisher=Ismael Osuna
AppPublisherURL=https://github.com/ismaelosuna7824/gentle-shell-gpui
AppSupportURL=https://github.com/ismaelosuna7824/gentle-shell-gpui/issues
AppUpdatesURL=https://github.com/ismaelosuna7824/gentle-shell-gpui/releases
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\Gentle Shell
DisableProgramGroupPage=yes
DisableDirPage=auto
OutputDir={#OutputDir}
OutputBaseFilename=gentle-shell-gpui-{#AppVersion}-windows-x86_64-setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\gentle-shell-gpui.exe
UninstallDisplayName=Gentle Shell
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; Close a running Gentle Shell before replacing it, and start it again after.
CloseApplications=yes
RestartApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Gentle Shell"; Filename: "{app}\gentle-shell-gpui.exe"
Name: "{autodesktop}\Gentle Shell"; Filename: "{app}\gentle-shell-gpui.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\gentle-shell-gpui.exe"; Description: "{cm:LaunchProgram,Gentle Shell}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The previous exe an in-app update leaves until the next start.
Type: files; Name: "{app}\gentle-shell-gpui.exe.old"
