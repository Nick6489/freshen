; Build from repository root with /DAppVersion=1.0.0 /DSourceDir=... /DFreshenInnoDll=...
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#ifndef SourceDir
  #error Define SourceDir containing inno_host.exe and freshen-helper.exe
#endif
[Setup]
AppId=FreshenInnoExample
AppName=Freshen Inno Example
AppVersion={#AppVersion}
DefaultDirName={localappdata}\FreshenInnoExample
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\inno_host.exe
OutputDir=..\target\inno-installers
OutputBaseFilename=FreshenExample-{#AppVersion}
Compression=lzma2
SolidCompression=yes
CloseApplications=no
RestartApplications=no
[Files]
Source: "{#SourceDir}\inno_host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\freshen-helper.exe"; DestDir: "{app}"; Flags: ignoreversion
#include "FreshenInno.iss"
