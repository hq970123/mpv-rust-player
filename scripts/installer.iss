; Inno Setup script. CI passes /DSourceDir=... /DOutDir=...
#define AppName "Filmora Player"
#define AppVersion "0.1.0"

[Setup]
AppId={{6B0D6E39-3C64-4F0B-9A7E-4E1D2B7C9F21}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Filmora Player
DefaultDirName={autopf}\FilmoraPlayer
DefaultGroupName={#AppName}
OutputDir={#OutDir}
OutputBaseFilename=FilmoraPlayer-windows-x64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog
WizardStyle=modern

[Languages]
Name: "chs"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\FilmoraPlayer.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\FilmoraPlayer.exe"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\shell\open\command"; ValueType: string; ValueData: """{app}\FilmoraPlayer.exe"" ""%1"""; Flags: uninsdeletekey

[Run]
Filename: "{app}\FilmoraPlayer.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
