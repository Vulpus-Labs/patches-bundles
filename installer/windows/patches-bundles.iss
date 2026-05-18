; Inno Setup script for the Patches stdlib bundles.
;
; Compiles with Inno Setup 6.x. The .pxm files are produced by
; `cargo xtask package` and live in <repo>\release\plugins\.

#define AppName    "Patches Stdlib Bundles"
#define Publisher  "Vulpus Labs"
#define AppURL     "https://github.com/Vulpus-Labs/patches-bundles"

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define BinDir "..\..\release\plugins"

[Setup]
; Keep this AppId stable across releases so upgrades replace prior installs.
AppId={{8E2F5C7A-3D4B-4A1E-9F8C-7B5D3E1A2C9F}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#Publisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
; Host scans %APPDATA%\Patches\data\bundles by default (ProjectDirs data_dir).
; Per-user, no admin required.
DefaultDirName={userappdata}\Patches\data\bundles
DisableDirPage=no
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName} {#AppVersion}
OutputBaseFilename=patches-bundles-{#AppVersion}-windows-x64
OutputDir=Output
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
WizardStyle=modern
LicenseFile=..\..\LICENSE
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Files]
Source: "{#BinDir}\patches_vintage.pxm";    DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\patches_drums.pxm";      DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\patches_fft_bundle.pxm"; DestDir: "{app}"; Flags: ignoreversion

[Messages]
SelectDirDesc=Choose the directory the Patches host scans for .pxm bundles. The default is the per-user data directory (%APPDATA%\Patches\data\bundles), which the host scans automatically. Change it only if you have pointed the host at a different directory.
SelectDirLabel3=Setup will install the [name] .pxm files into the following folder.
