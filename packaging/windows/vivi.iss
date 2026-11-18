; Inno Setup script for the per-user Windows installer
; (vivi-X.Y.Z-windows-x64-setup.exe / vivi-X.Y.Z-windows-arm64-setup.exe). Built by packaging/windows/build-installer.ps1,
; which stages vivi.exe, libmpv-2.dll, the app-local VC++ runtime DLLs and this folder's
; vivi.ico into StageDir, then runs ISCC with the defines below.
;
; Installs to %LOCALAPPDATA%\Programs\Vivi without admin rights, adds a Start menu
; shortcut (and optionally a desktop one) and registers the uninstaller in Settings > Apps.
; Settings, the library and caches (%USERPROFILE%\.config\vivi etc.) are left on uninstall.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef NameVersion
  #define NameVersion "dev"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef StageDir
  #define StageDir "stage"
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
; Fixed for every version: the same AppId makes a new installer upgrade the old install.
AppId={{B9287A0D-104B-47DD-A6DA-D9551641F5A7}
AppName=Vivi
AppVersion={#AppVersion}
AppVerName=Vivi {#NameVersion}
AppPublisher=Popelyshyn Ruslan
AppPublisherURL=https://viviplayer.app
AppSupportURL=https://github.com/Loafer19/vivi/issues
AppUpdatesURL=https://github.com/Loafer19/vivi/releases
AppCopyright=MIT License
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Vivi
VersionInfoDescription=Vivi Setup
; Per-user by default ({autopf} = %LOCALAPPDATA%\Programs); /ALLUSERS on the command line
; installs for everyone (Program Files, needs admin).
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
DefaultDirName={autopf}\Vivi
DefaultGroupName=Vivi
DisableProgramGroupPage=yes
DisableDirPage=auto
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
; x64compatible: also Windows 11 on Arm (x64 emulation), though the arm64 installer is native there.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=vivi-{#NameVersion}-windows-{#Arch}-setup
SetupIconFile=vivi.ico
UninstallDisplayIcon={app}\vivi.ico
UninstallDisplayName=Vivi
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running Vivi (Restart Manager) before files are replaced or removed.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Vivi"; Filename: "{app}\vivi.exe"; IconFilename: "{app}\vivi.ico"; Comment: "Media player"
Name: "{autodesktop}\Vivi"; Filename: "{app}\vivi.exe"; IconFilename: "{app}\vivi.ico"; Comment: "Media player"; Tasks: desktopicon

[Registry]
; "vivi" in Win+R and Start > Run finds the installed exe.
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\vivi.exe"; ValueType: string; ValueName: ""; ValueData: "{app}\vivi.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\vivi.exe"; ValueType: string; ValueName: "Path"; ValueData: "{app}"

[Run]
Filename: "{app}\vivi.exe"; Description: "{cm:LaunchProgram,Vivi}"; Flags: nowait postinstall skipifsilent
