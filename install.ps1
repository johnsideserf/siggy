$ErrorActionPreference = "Stop"

$Repo = "johnsideserf/signal-tui"
$SignalCliRepo = "AsamK/signal-cli"
$Target = "x86_64-pc-windows-msvc"
$InstallDir = "$env:LOCALAPPDATA\signal-tui"

function Info($msg) { Write-Host ":: $msg" -ForegroundColor Blue }
function Err($msg) { Write-Host "error: $msg" -ForegroundColor Red; exit 1 }

# --- Get latest release ---
Info "Fetching latest release..."
try {
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
} catch {
    Err "Failed to fetch release info. Check your internet connection."
}

$Tag = $Release.tag_name
if (-not $Tag) { Err "Could not determine latest release tag" }
Info "Latest release: $Tag"

# --- Download and install signal-tui ---
$Archive = "signal-tui-$Tag-$Target.zip"
$DownloadUrl = "https://github.com/$Repo/releases/download/$Tag/$Archive"
$TmpDir = Join-Path $env:TEMP "signal-tui-install"

if (Test-Path $TmpDir) { Remove-Item -Recurse -Force $TmpDir }
New-Item -ItemType Directory -Path $TmpDir | Out-Null

Info "Downloading $Archive..."
try {
    Invoke-WebRequest -Uri $DownloadUrl -OutFile "$TmpDir\$Archive" -UseBasicParsing
} catch {
    Err "Download failed: $DownloadUrl"
}

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir | Out-Null
}

Info "Extracting..."
Expand-Archive -Path "$TmpDir\$Archive" -DestinationPath $TmpDir -Force
Copy-Item "$TmpDir\signal-tui.exe" "$InstallDir\signal-tui.exe" -Force

Info "Installed signal-tui to $InstallDir\signal-tui.exe"

# --- Add to PATH ---
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$InstallDir;$UserPath", "User")
    Info "Added $InstallDir to user PATH"
} else {
    Info "$InstallDir already in PATH"
}

# --- Check for signal-cli ---
$SignalCli = Get-Command signal-cli -ErrorAction SilentlyContinue
if ($SignalCli) {
    Info "signal-cli found: $($SignalCli.Source)"
} else {
    Info "signal-cli not found"

    # Check for Java
    $Java = Get-Command java -ErrorAction SilentlyContinue
    if ($Java) {
        Info "Java found, installing signal-cli..."

        try {
            $ScliRelease = Invoke-RestMethod -Uri "https://api.github.com/repos/$SignalCliRepo/releases/latest"
        } catch {
            Err "Failed to fetch signal-cli release info"
        }

        $ScliTag = $ScliRelease.tag_name
        $ScliVersion = $ScliTag.TrimStart("v")
        $ScliArchive = "signal-cli-$ScliVersion.tar.gz"
        $ScliUrl = "https://github.com/$SignalCliRepo/releases/download/$ScliTag/$ScliArchive"

        Info "Downloading signal-cli $ScliTag..."
        try {
            Invoke-WebRequest -Uri $ScliUrl -OutFile "$TmpDir\$ScliArchive" -UseBasicParsing
        } catch {
            Err "signal-cli download failed: $ScliUrl"
        }

        Info "Extracting signal-cli..."
        tar xzf "$TmpDir\$ScliArchive" -C $TmpDir

        $ScliDir = "$InstallDir\signal-cli"
        if (Test-Path $ScliDir) { Remove-Item -Recurse -Force $ScliDir }
        Copy-Item "$TmpDir\signal-cli-$ScliVersion" $ScliDir -Recurse

        # Create a wrapper batch file
        $WrapperContent = "@echo off`r`njava -jar `"%~dp0signal-cli\lib\signal-cli.jar`" %*"
        Set-Content -Path "$InstallDir\signal-cli.bat" -Value $WrapperContent

        Info "Installed signal-cli to $ScliDir"
    } else {
        Write-Host ""
        Info "signal-cli requires Java 21+. Install Java from:"
        Write-Host ""
        Write-Host "  https://adoptium.net/"
        Write-Host ""
        Info "Then re-run this script to install signal-cli."
        Write-Host ""
    }
}

# --- Cleanup ---
Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue

# --- Done ---
Write-Host ""
Info "Done! Restart your terminal, then run 'signal-tui' to get started."
