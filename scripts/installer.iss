; Inno Setup script. CI passes /DSourceDir=... /DOutDir=...
#define AppName "Filmora Player"
#define AppVersion "0.1.0"

[Setup]
ChangesAssociations=yes
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
Name: "chs"; MessagesFile: "lang\ChineseSimplified.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "assoc"; Description: "Add Filmora Player to 'Open with' for video / audio files"; GroupDescription: "File associations:"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\FilmoraPlayer.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\FilmoraPlayer.exe"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\shell\open\command"; ValueType: string; ValueData: """{app}\FilmoraPlayer.exe"" ""%1"""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\FilmoraPlayer.Media"; ValueType: string; ValueData: "Filmora Player Media"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\FilmoraPlayer.Media\DefaultIcon"; ValueType: string; ValueData: "{app}\FilmoraPlayer.exe,0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\FilmoraPlayer.Media\shell\open\command"; ValueType: string; ValueData: """{app}\FilmoraPlayer.exe"" ""%1"""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mkv\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mkv"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mp4\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m4v\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".m4v"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mov\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mov"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.avi\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".avi"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.wmv\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".wmv"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.flv\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".flv"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.webm\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".webm"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.ts\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".ts"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m2ts\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".m2ts"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mts\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mts"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mpg\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mpg"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mpeg\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mpeg"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.vob\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".vob"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.3gp\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".3gp"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.ogv\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".ogv"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.rmvb\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".rmvb"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.rm\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".rm"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.asf\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".asf"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.f4v\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".f4v"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.divx\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".divx"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mp3\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mp3"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.flac\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".flac"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.m4a\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".m4a"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.aac\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".aac"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.ogg\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".ogg"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.opus\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".opus"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.wav\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".wav"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.wma\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".wma"; ValueData: ""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.ape\OpenWithProgids"; ValueType: string; ValueName: "FilmoraPlayer.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\FilmoraPlayer.exe\SupportedTypes"; ValueType: string; ValueName: ".ape"; ValueData: ""; Tasks: assoc

[Run]
Filename: "{app}\FilmoraPlayer.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
