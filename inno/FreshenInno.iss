; Define FreshenInnoDll to the DLL matching the INSTALLER process architecture.
; Include at top level. Event attributes coexist with application handlers.
#ifndef FreshenInnoDll
  #error Define FreshenInnoDll to the absolute path of FreshenInno.dll
#endif

[Files]
Source: "{#FreshenInnoDll}"; Flags: dontcopy noencryption

[Code]
function FreshenConnect(PipeName, Token: String): Integer;
  external 'FreshenInnoConnect@files:FreshenInno.dll stdcall delayload';
function FreshenPhase(Phase: Cardinal): Integer;
  external 'FreshenInnoPhase@files:FreshenInno.dll stdcall delayload';
function FreshenProgress(Current, Total: Cardinal): Integer;
  external 'FreshenInnoProgress@files:FreshenInno.dll stdcall delayload';
procedure FreshenDisconnect;
  external 'FreshenInnoDisconnect@files:FreshenInno.dll stdcall delayload';

var FreshenActive: Boolean;

procedure FreshenCheck(ResultCode: Integer);
begin
  if ResultCode <> 0 then begin
    FreshenActive := False;
    Log('Freshen telemetry unavailable; installation continues.');
    try FreshenDisconnect; except end;
  end;
end;

<event('InitializeSetup')>
function FreshenInitialize: Boolean;
var PipeName, Token: String;
begin
  Result := True;
  FreshenActive := False;
  PipeName := ExpandConstant('{param:FRESHENPIPE|}');
  Token := ExpandConstant('{param:FRESHENTOKEN|}');
  if (PipeName = '') or (Token = '') then exit;
  try
    FreshenActive := FreshenConnect(PipeName, Token) = 0;
    if FreshenActive then FreshenCheck(FreshenPhase(1));
  except
    Log('Freshen bridge could not load or connect; installation continues.');
  end;
end;

<event('CurStepChanged')>
procedure FreshenStepChanged(CurStep: TSetupStep);
begin
  if not FreshenActive then exit;
  try
    if CurStep = ssInstall then FreshenCheck(FreshenPhase(2));
    if CurStep = ssPostInstall then FreshenCheck(FreshenPhase(3));
  except FreshenActive := False; end;
end;

<event('CurInstallProgressChanged')>
procedure FreshenInstallProgressChanged(CurProgress, MaxProgress: Integer);
begin
  if not FreshenActive then exit;
  if (CurProgress < 0) or (MaxProgress <= 0) then exit;
  try FreshenCheck(FreshenProgress(CurProgress, MaxProgress));
  except FreshenActive := False; end;
end;

<event('DeinitializeSetup')>
procedure FreshenDeinitialize;
begin
  if FreshenActive then begin
    try FreshenDisconnect; except end;
  end;
  FreshenActive := False;
end;
