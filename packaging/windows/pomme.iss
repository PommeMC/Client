; Installs a release bundle (the files of pomme-client-windows-x64.zip) for the
; current user, into a folder the client's updater can write to.
;
; usage: iscc /DAppVersion=<x.y.z> /DBundle=<bundle-dir> /DOutDir=<out-dir> /DOutName=<name> pomme.iss

[Setup]
AppId={{22F91093-7D94-4FA4-9045-1F5CBA3D82D5}
AppName=Pomme
AppVersion={#AppVersion}
AppPublisher=PommeMC
AppPublisherURL=https://github.com/PommeMC/Client
DefaultDirName={localappdata}\Programs\Pomme
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename={#OutName}
SetupIconFile=..\..\pomme-client\assets\icon.ico
UninstallDisplayIcon={app}\pomme-client.exe
WizardStyle=modern
Compression=lzma2
SolidCompression=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Bundle}\*"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Pomme"; Filename: "{app}\pomme-client.exe"
Name: "{autodesktop}\Pomme"; Filename: "{app}\pomme-client.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\pomme-client.exe"; Description: "{cm:LaunchProgram,Pomme}"; Flags: nowait postinstall skipifsilent

; What the updater leaves behind; the data folder (%LOCALAPPDATA%\Pomme) stays.
[UninstallDelete]
Type: files; Name: "{app}\*.old"
Type: filesandordirs; Name: "{app}\.pomme-update"
