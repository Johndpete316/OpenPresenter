; PP6 Windows installer (Inno Setup 6). Built by packaging/build-installer.sh, which
; passes /DStage=<folder with the files> /DVersion=0.1.0 /DBuild=<git commit>.
;
; Installing a newer installer over an older one upgrades in place (same AppId).
; Settings, playlists, logs and caches live in %LOCALAPPDATA%\PP6 and %APPDATA%\PP6:
; installing, upgrading and uninstalling never touch them.

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
AppId={{2D90CC4E-10A8-44D9-80F1-7A36A62E1357}
AppName=PP6
AppVersion={#Version} ({#Build})
AppVerName=PP6 {#Version}
VersionInfoVersion={#Version}
DefaultDirName={autopf}\PP6
DefaultGroupName=PP6
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#Stage}\..
OutputBaseFilename=PP6-Setup-{#Version}-{#Build}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\pp6.exe
SetupIconFile=..\crates\app\assets\pp6.ico
; If pp6 is running, the installer offers to close it (Restart Manager).
CloseApplications=yes
RestartApplications=no
SetupLogging=yes

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#Stage}\pp6.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\pp6tool.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\libmpv-2.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\PP6"; Filename: "{app}\pp6.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\PP6"; Filename: "{app}\pp6.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\pp6.exe"; Description: "Start PP6"; Flags: nowait postinstall skipifsilent
