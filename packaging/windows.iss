[Setup]
AppId={{7BEE9937-01B7-4A61-A74C-90D5A19FC5C8}
AppName=Parally
AppVersion=0.5.2
DefaultDirName={localappdata}\Programs\Parally
DefaultGroupName=Parally
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Parally-0.5.2-windows-x64-setup
SetupIconFile=..\assets\parally.ico
UninstallDisplayIcon={app}\parally.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Files]
Source: "..\target\release\parallel-downloader.exe"; DestDir: "{app}\release-0.4"; DestName: "Parally.exe"; Flags: ignoreversion
Source: "..\browser\extension\*"; DestDir: "{app}\browser\extension"; Flags: ignoreversion recursesubdirs
Source: "..\assets\parally.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Parally"; Filename: "{app}\release-0.4\Parally.exe"; IconFilename: "{app}\parally.ico";

[Run]
Filename: "{app}\release-0.4\Parally.exe"; Description: "{cm:LaunchProgram,Parally}"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\release-0.4\Parally.exe"; Parameters: "--disconnect-browsers"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveParallyHost"

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Startup: String;
begin
  if CurUninstallStep = usUninstall then begin
    if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Parally', Startup)
      and ((CompareText(Startup, '"' + ExpandConstant('{app}\release-0.4\Parally.exe') + '" --tray') = 0)
        or (CompareText(Startup, '"' + ExpandConstant('{app}\release-0.4\Parally.exe') + '"') = 0)) then
      RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Parally');
  end;
end;
