; Build with scripts/build-demo.ps1. This is a new, isolated per-user test identity.
#ifndef SourceDir
  #error Define SourceDir
#endif
#ifndef AppVersion
  #error Define AppVersion
#endif
#ifndef OutputDir
  #error Define OutputDir
#endif
[Setup]
AppId=FreshenNativeDialogTest
AppName=Freshen Native Test
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\FreshenNativeTest
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=FreshenNativeTest-{#AppVersion}-x64
UninstallDisplayIcon={app}\freshen-native-test.exe
Compression=lzma2
SolidCompression=yes
CloseApplications=no
RestartApplications=no
[Files]
Source: "{#SourceDir}\freshen-native-test.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\freshen-helper.exe"; DestDir: "{app}"; Flags: ignoreversion
[Icons]
Name: "{autoprograms}\Freshen Native Test"; Filename: "{app}\freshen-native-test.exe"
[Run]
Filename: "{app}\freshen-native-test.exe"; Description: "Open Freshen Native Test"; Flags: nowait postinstall skipifsilent
#include "..\..\inno\FreshenInno.iss"
[Code]
{ Deliberate test-only work makes disabled cancellation and finalization observable.
  It does not block the Freshen UI and is absent during manual installation. }
<event('CurStepChanged')>
procedure NativeTestDelay(Step: TSetupStep);
begin
  if ExpandConstant('{param:FRESHENPIPE|}') = '' then exit;
  if Step = ssInstall then Sleep(1800);
  if Step = ssPostInstall then Sleep(2200);
end;
