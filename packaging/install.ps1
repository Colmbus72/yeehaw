# Yeehaw CLI installer (Windows)
# Usage: powershell -c "irm https://yeehaw.cool/install.ps1 | iex"

$ErrorActionPreference = 'Stop'

$Repo = 'Colmbus72/yeehaw'
$Bin  = 'yeehaw'
$InstallDir = if ($env:YEEHAW_INSTALL_DIR) { $env:YEEHAW_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\yeehaw' }

# ---- detect target ------------------------------------------------------

$arch = switch ((Get-CimInstance Win32_Processor).Architecture) {
  9 { 'x86_64' }     # AMD64 / x64
  12 { 'aarch64' }   # ARM64
  default { throw "Unsupported CPU architecture" }
}

# We currently only ship x86_64 windows; arm64 falls back to that under emulation
if ($arch -eq 'aarch64') { $arch = 'x86_64' }

$target = "$arch-pc-windows-msvc"
$asset  = "$Bin-$target.zip"
$url    = "https://github.com/$Repo/releases/latest/download/$asset"

Write-Host ":: installing $Bin for $target" -ForegroundColor Cyan

# ---- download & extract -------------------------------------------------

$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "yeehaw-install-$([guid]::NewGuid())") -Force
try {
  $zipPath = Join-Path $tmp.FullName $asset

  Write-Host ":: downloading $url" -ForegroundColor Cyan
  Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing

  Write-Host ":: extracting" -ForegroundColor Cyan
  Expand-Archive -Path $zipPath -DestinationPath $tmp.FullName -Force

  $extractedBin = Join-Path $tmp.FullName "$Bin-$target\$Bin.exe"
  if (-not (Test-Path $extractedBin)) {
    throw "binary not found in archive at $extractedBin"
  }

  # ---- install ----------------------------------------------------------

  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  $dest = Join-Path $InstallDir "$Bin.exe"
  Copy-Item -Path $extractedBin -Destination $dest -Force

  Write-Host "✓ installed $Bin → $dest" -ForegroundColor Green

  # ---- PATH check -------------------------------------------------------

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if ($userPath -notlike "*$InstallDir*") {
    Write-Host ""
    Write-Host "Adding $InstallDir to your user PATH..." -ForegroundColor Yellow
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$InstallDir", 'User')
    Write-Host "✓ PATH updated. Open a new terminal for it to take effect." -ForegroundColor Green
  }

  Write-Host ""
  Write-Host "Run " -NoNewline
  Write-Host "$Bin" -NoNewline -ForegroundColor White
  Write-Host " to get started."
} finally {
  Remove-Item -Path $tmp.FullName -Recurse -Force -ErrorAction SilentlyContinue
}
