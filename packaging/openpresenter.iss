; OpenPresenter Windows installer (Inno Setup 6). Built by packaging/build-installer.sh,
; which passes /DStage=<folder with the files> /DVersion=0.1.0 /DBuild=<git commit>.
;
; Installing a newer installer over an older one upgrades in place (same AppId).
; Settings, playlists, logs and caches live in %LOCALAPPDATA%\OpenPresenter and
; %APPDATA%\OpenPresenter: installing, upgrading and uninstalling never touch them.
;
; This is a new AppId, not the one the old "PP6" builds used: OpenPresenter installs
; alongside (not over) an old PP6 install. Uninstall PP6 first (Settings → Apps).

#ifndef Stage
  #error Pass /DStage=<folder>
#endif
#ifndef Version
  #error Pass /DVersion=<x.y.z>
#endif
#ifndef Build
  #define Build "unknown"
#endif

[Setup]
; Never change AppId: it's how Windows knows a new installer is an upgrade.
AppId={{264BA871-2AAF-48D9-ADF5-36E230C7C1FA}
AppName=OpenPresenter
AppVersion={#Version} ({#Build})
AppVerName=OpenPresenter {#Version}
AppPublisher=John Peterson
AppCopyright=Copyright (C) 2026 John Peterson. GPL-3.0.
VersionInfoVersion={#Version}
VersionInfoProductName=OpenPresenter
VersionInfoDescription=OpenPresenter Setup
DefaultDirName={autopf}\OpenPresenter
DefaultGroupName=OpenPresenter
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#Stage}\..
OutputBaseFilename=OpenPresenter-Setup-{#Version}-{#Build}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; GPL-3.0 licence page (informational: the GPL doesn't require accepting it to run).
LicenseFile={#Stage}\LICENSE.txt
UninstallDisplayName=OpenPresenter
UninstallDisplayIcon={app}\openpresenter.exe
SetupIconFile=..\docs\branding\windows\openpresenter.ico
WizardSmallImageFile=..\docs\branding\windows\wizard-small-55x58.bmp,..\docs\branding\windows\wizard-small-110x116.bmp
; If OpenPresenter is running, the installer offers to close it (Restart Manager).
CloseApplications=yes
RestartApplications=no
SetupLogging=yes

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#Stage}\openpresenter.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\pp6tool.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\libmpv-2.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\OpenPresenter"; Filename: "{app}\openpresenter.exe"; WorkingDir: "{app}"; Comment: "Worship presentation software"
Name: "{autodesktop}\OpenPresenter"; Filename: "{app}\openpresenter.exe"; WorkingDir: "{app}"; Tasks: desktopicon; Comment: "Worship presentation software"

[Run]
Filename: "{app}\openpresenter.exe"; Description: "Start OpenPresenter"; Flags: nowait postinstall skipifsilent
